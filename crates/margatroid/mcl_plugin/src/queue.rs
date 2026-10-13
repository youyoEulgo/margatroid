use crate::parser::{self, ParsedOperation};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{Entity, World};
use margatroid_types::agent_components::{Agent, AgentState, WorldAgentExt};
use margatroid_types::block_types::{BlockInner, InnerType};
use margatroid_types::execution_components::ExecutionActivityKind;
use margatroid_types::history_components::{HistoryError, HistoryRecord};
use margatroid_types::inference_events::InferenceRequest;
use margatroid_types::lua_runtime_components::DriverWakeSlot;
use margatroid_types::mcl_events::*;
use margatroid_types::message_types::{AgentMessage, ToolCall};
use margatroid_types::resource_components::CallableResource;
use margatroid_types::resource_events::*;
use margatroid_types::tool_events::ToolExecuteRequest;
use resource_id_plugin::ResourceID;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

fn stopped(agent: &Agent) -> bool {
    matches!(agent.state, AgentState::Stopping { .. })
        || agent.execution.is_closed()
        || !agent.mcl.accepting
        || agent.mcl.drained.is_closed()
}

pub fn enqueue_mcl_command(world: &mut World, request: MclCommand) {
    let checked = (|| {
        if request.id.trim().is_empty()
            || request.source.resource_type() != "client"
                && request.source.resource_type() != "agent"
        {
            return Err(MclError::InvalidRequest);
        }
        let entity = world
            .agent_by_id(&request.target)
            .map_err(|_| MclError::AgentMissing)?;
        let agent = world
            .get_component_mut::<Agent>(entity)
            .ok_or(MclError::AgentMissing)?;
        if stopped(agent) {
            return Err(MclError::AgentStopping);
        }
        if agent
            .mcl
            .commands
            .iter()
            .any(|entry| entry.id == request.id)
            || agent
                .mcl
                .pending
                .as_ref()
                .is_some_and(|p| p.command_id == request.id)
        {
            return Err(MclError::DuplicateRequest);
        }
        agent
            .mcl
            .drained
            .mark_busy()
            .map_err(|_| MclError::AgentStopping)?;
        agent.mcl.commands.push_back(request.clone());
        Ok(())
    })();
    if let Err(error) = checked {
        crate::runtime::publish_result(world, MclCommandResult::from_request(&request, Err(error)));
    }
}

fn history_record(
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    kind: &str,
    suffix: &str,
    payload: serde_json::Value,
) -> HistoryRecord {
    HistoryRecord {
        key: format!("{entity:?}:{}:{kind}:{suffix}", request.id),
        source: request.source.to_string(),
        turn,
        kind: kind.into(),
        content: if kind == "mcl" {
            request.command.text.clone()
        } else {
            payload.to_string()
        },
        payload,
    }
}

fn record_command(
    world: &World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    cancelled: bool,
) -> Result<(), HistoryError> {
    let agent = world
        .get_component::<Agent>(entity)
        .ok_or_else(|| HistoryError::WriteFailed("agent missing".into()))?;
    agent.history.append(history_record(entity, request, turn, "mcl", "command", serde_json::json!({"text":request.command.text,"arguments":request.command.arguments,"cancelled":cancelled})))
}

fn contains_system(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::System { .. } => true,
        AgentMessage::Inject { messages } => messages.iter().any(contains_system),
        _ => false,
    }
}

fn append_messages(
    world: &World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    messages: &[AgentMessage],
) -> Result<(), HistoryError> {
    let agent = world
        .get_component::<Agent>(entity)
        .ok_or_else(|| HistoryError::WriteFailed("agent missing".into()))?;
    let records = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            history_record(
                entity,
                request,
                turn.clone(),
                "message",
                &index.to_string(),
                serde_json::to_value(message).unwrap(),
            )
        })
        .collect();
    agent.history.append_batch(records)
}

pub fn complete_mcl_async(world: &mut World, output: MclAsyncOutput) {
    let entity = output.pending.target_entity;
    let matches = world.get_component::<Agent>(entity).is_some_and(|agent| {
        agent.mcl.pending.as_ref().is_some_and(|p| {
            p.command_id == output.pending.command_id
                && p.operation_id == output.pending.operation_id
                && p.target_entity == entity
        }) && agent.mcl.commands.front() == Some(&output.request)
    });
    if !matches {
        return;
    }
    let history_error = match &output.result {
        Ok(MclValue::Inference(response)) => append_messages(
            world,
            entity,
            &output.request,
            output.turn.clone(),
            std::slice::from_ref(&response.message),
        )
        .err(),
        Ok(MclValue::Array(BlockInner::Message(messages))) => append_messages(
            world,
            entity,
            &output.request,
            output.turn.clone(),
            messages,
        )
        .err(),
        _ => None,
    };
    let mut reply = MclCommandResult::from_request(&output.request, output.result);
    reply.history_error = history_error;
    crate::runtime::publish_result(world, reply);
    finish_head(world, entity);
}

fn finish_head(world: &mut World, entity: Entity) {
    if let Some(agent) = world.get_component_mut::<Agent>(entity) {
        agent.mcl.commands.pop_front();
        agent.mcl.pending = None;
        agent.mcl.drained.set_pending_cancel(None);
        if agent.mcl.commands.is_empty() {
            agent.mcl.drained.mark_drained();
        }
    }
}

fn cancel_source_commands(world: &mut World, entity: Entity) {
    let Some(agent) = world.get_component::<Agent>(entity) else {
        return;
    };
    let cancelled = agent
        .mcl
        .commands
        .iter()
        .filter(|request| crate::runtime::driver_command_cancelled(world, request))
        .cloned()
        .collect::<Vec<_>>();
    let turn = agent.turn.clone();
    for request in cancelled {
        let agent = world.get_component_mut::<Agent>(entity).unwrap();
        if let Some(pending) = agent
            .mcl
            .pending
            .as_ref()
            .filter(|pending| pending.command_id == request.id)
        {
            pending.cancel.cancel();
            continue;
        }
        let history_error = record_command(world, entity, &request, turn.clone(), true).err();
        let mut reply = MclCommandResult::from_request(&request, Err(MclError::Cancelled));
        reply.history_error = history_error;
        crate::runtime::publish_result(world, reply);
        world
            .get_component_mut::<Agent>(entity)
            .unwrap()
            .mcl
            .commands
            .retain(|entry| entry != &request);
    }
    let agent = world.get_component_mut::<Agent>(entity).unwrap();
    if agent.mcl.commands.is_empty() && agent.mcl.pending.is_none() {
        agent.mcl.drained.mark_drained();
    }
}

pub fn process_mcl_queue(world: &mut World, entity: Entity) {
    cancel_source_commands(world, entity);
    if world.get_component::<Agent>(entity).is_some_and(stopped) {
        let (cancelled, turn) = {
            let agent = world.get_component_mut::<Agent>(entity).unwrap();
            agent.mcl.accepting = false;
            agent.mcl.drained.close();
            if let Some(pending) = &agent.mcl.pending {
                pending.cancel.cancel();
            }
            let keep = usize::from(agent.mcl.pending.is_some());
            let cancelled = agent.mcl.commands.split_off(keep);
            (cancelled, agent.turn.clone())
        };
        for request in cancelled {
            let history_error = record_command(world, entity, &request, turn.clone(), true).err();
            let mut reply = MclCommandResult::from_request(&request, Err(MclError::Cancelled));
            reply.history_error = history_error;
            crate::runtime::publish_result(world, reply);
        }
        let agent = world.get_component_mut::<Agent>(entity).unwrap();
        if agent.mcl.commands.is_empty() && agent.mcl.pending.is_none() {
            agent.mcl.drained.mark_drained();
        }
        return;
    }
    for _ in 0..64 {
        let Some(agent) = world.get_component::<Agent>(entity) else {
            return;
        };
        if agent.mcl.pending.is_some() {
            return;
        }
        let Some(request) = agent.mcl.commands.front().cloned() else {
            return;
        };
        let turn = agent.turn.clone();
        let parsed = parser::parse(&request.command.text, &request.command.arguments);
        let prepared_append = match &parsed {
            Ok(ParsedOperation::Effect { name, parameters }) if name == "history_append" => Some(
                prepare_history_append(world, entity, parameters, &request.command.arguments),
            ),
            _ => None,
        };
        let is_append = matches!(&prepared_append, Some(Ok(_)));
        if !is_append {
            if let Err(error) = record_command(world, entity, &request, turn.clone(), false) {
                crate::runtime::publish_result(
                    world,
                    MclCommandResult::from_request(
                        &request,
                        Err(MclError::HistoryWriteFailed(error.to_string())),
                    ),
                );
                finish_head(world, entity);
                continue;
            }
        }
        let result = match prepared_append {
            Some(Ok(messages)) => append_messages(world, entity, &request, turn, &messages)
                .map(|_| Some(MclValue::Unit))
                .map_err(|e| MclError::HistoryWriteFailed(e.to_string())),
            Some(Err(error)) => Err(error),
            None => match parsed {
                Err(error) => Err(error),
                Ok(operation) => execute(world, entity, &request, turn, operation),
            },
        };
        match result {
            Ok(None) => {
                if world
                    .get_component::<Agent>(entity)
                    .is_some_and(|agent| agent.mcl.pending.is_some())
                {
                    return;
                }
                continue;
            }
            result => {
                crate::runtime::publish_result(
                    world,
                    MclCommandResult::from_request(&request, result.map(|v| v.unwrap())),
                );
                finish_head(world, entity);
            }
        }
    }
}

fn prepare_history_append(
    world: &World,
    entity: Entity,
    parameters: &[parser::ArrayParameter],
    arguments: &[serde_json::Value],
) -> Result<Vec<AgentMessage>, MclError> {
    let agent = world
        .get_component::<Agent>(entity)
        .ok_or(MclError::AgentMissing)?;
    let arrays = parser::evaluate_effect_arguments_with(
        &|alias| {
            world
                .agent_get_import(&agent.resourceid, alias)
                .map_err(|_| MclError::AliasMissing)
        },
        &agent.block,
        "history_append",
        parameters,
        arguments,
    )?;
    let Some(BlockInner::Message(messages)) = arrays.into_iter().next() else {
        return Err(MclError::TypeMismatch);
    };
    if messages.iter().any(contains_system) {
        return Err(MclError::TypeMismatch);
    }
    Ok(messages)
}

fn execute(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    operation: ParsedOperation,
) -> Result<Option<MclValue>, MclError> {
    match operation {
        ParsedOperation::Create { block_id, fields } => {
            parser::create_block(
                &mut world
                    .get_component_mut::<Agent>(entity)
                    .ok_or(MclError::AgentMissing)?
                    .block,
                &block_id,
                &fields,
            )?;
            Ok(Some(MclValue::Unit))
        }
        ParsedOperation::Get { expression } => {
            let agent = world
                .get_component::<Agent>(entity)
                .ok_or(MclError::AgentMissing)?;
            let value = parser::select_array_with(&agent.block, &expression, &|alias| {
                world
                    .agent_get_import(&agent.resourceid, alias)
                    .map_err(|_| MclError::AliasMissing)
            })?;
            Ok(Some(MclValue::Array(value)))
        }
        ParsedOperation::Inject { source, target } => {
            let agent = world
                .get_component::<Agent>(entity)
                .ok_or(MclError::AgentMissing)?;
            let prepared = parser::prepare_inject_with(
                &|alias| {
                    world
                        .agent_get_import(&agent.resourceid, alias)
                        .map_err(|_| MclError::AliasMissing)
                },
                &agent.block,
                &source,
                &target,
                &request.command.arguments,
            )?;
            world
                .get_component_mut::<Agent>(entity)
                .unwrap()
                .block
                .replace(
                    &prepared.path,
                    prepared.start,
                    prepared.end,
                    prepared.replacement,
                )
                .map_err(|_| MclError::TypeMismatch)?;
            Ok(Some(MclValue::Unit))
        }
        ParsedOperation::Import {
            kind,
            resource_id,
            alias,
        } => begin_import(world, entity, request, turn, kind, resource_id, alias),
        ParsedOperation::Effect { name, parameters } => {
            let agent_id = world
                .get_component::<Agent>(entity)
                .ok_or(MclError::AgentMissing)?
                .resourceid
                .clone();
            let arrays = parser::evaluate_effect_arguments_with(
                &|alias| {
                    world
                        .agent_get_import(&agent_id, alias)
                        .map_err(|_| MclError::AliasMissing)
                },
                &world
                    .get_component::<Agent>(entity)
                    .ok_or(MclError::AgentMissing)?
                    .block,
                &name,
                &parameters,
                &request.command.arguments,
            )?;
            match name.as_str() {
                "inference" => begin_inference(world, entity, request, turn, arrays),
                "tool_call" => begin_tools(world, entity, request, turn, arrays),
                "history_append" => {
                    let BlockInner::Message(messages) = &arrays[0] else {
                        return Err(MclError::TypeMismatch);
                    };
                    if messages.iter().any(contains_system) {
                        return Err(MclError::TypeMismatch);
                    }
                    append_messages(world, entity, request, turn, messages)
                        .map_err(|e| MclError::HistoryWriteFailed(e.to_string()))?;
                    Ok(Some(MclValue::Unit))
                }
                "sandbox_use" => {
                    let BlockInner::Sandbox(resources) = &arrays[0] else {
                        return Err(MclError::TypeMismatch);
                    };
                    world
                        .agent_switch_sandbox(&agent_id, resources.first().cloned())
                        .map_err(|e| MclError::ExecutionFailed(e.to_string()))?;
                    Ok(Some(MclValue::Unit))
                }
                "wake" => {
                    DriverWakeSlot::wake(&world.get_component::<Agent>(entity).unwrap().lua.wake)
                        .map_err(|e| MclError::ExecutionFailed(e.to_string()))?;
                    Ok(Some(MclValue::Unit))
                }
                "ready" => {
                    if !crate::runtime::driver_ready_authorized(world, entity, request) {
                        return Err(MclError::InvalidRequest);
                    }
                    let agent = world.get_component::<Agent>(entity).unwrap();
                    let Some(ready) = agent.lua.ready.as_ref() else {
                        return Ok(Some(MclValue::Text("ready already delivered".into())));
                    };
                    if ready.instance.owner != entity
                        || agent.lua.instance.as_ref() != Some(&ready.instance)
                    {
                        return Err(MclError::InvalidRequest);
                    }
                    Ok(Some(if ready.complete(Ok(())) {
                        MclValue::Unit
                    } else {
                        MclValue::Text("ready already delivered".into())
                    }))
                }
                "start_turn" => {
                    world.get_component_mut::<Agent>(entity).unwrap().new_turn();
                    Ok(Some(MclValue::Unit))
                }
                "finish" => {
                    world.get_component_mut::<Agent>(entity).unwrap().turn = None;
                    Ok(Some(MclValue::Unit))
                }
                _ => Err(MclError::EffectInvalid),
            }
        }
    }
}

fn pending(world: &mut World, entity: Entity, request: &MclCommand) -> MclPending {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let agent = world.get_component_mut::<Agent>(entity).unwrap();
    let pending = MclPending {
        command_id: request.id.clone(),
        target_entity: entity,
        operation_id: format!(
            "mcl:{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        cancel: agent.execution.child_cancel(),
    };
    agent.mcl.pending = Some(pending.clone());
    agent
        .mcl
        .drained
        .set_pending_cancel(Some(pending.cancel.clone()));
    pending
}

fn wait_result<F>(
    world: &World,
    request: MclCommand,
    pending: MclPending,
    turn: Option<String>,
    future: F,
) where
    F: std::future::Future<Output = Result<MclValue, MclError>> + Send + 'static,
{
    let events = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::spawn(future)
            .await
            .unwrap_or_else(|e| Err(MclError::ExecutionFailed(e.to_string())));
        events.send_event(MclAsyncOutput {
            request,
            pending,
            turn,
            result,
        });
    });
}

fn begin_inference(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    arrays: Vec<BlockInner>,
) -> Result<Option<MclValue>, MclError> {
    let mut arrays = arrays.into_iter();
    let Some(BlockInner::Message(messages)) = arrays.next() else {
        return Err(MclError::TypeMismatch);
    };
    let Some(BlockInner::Tool(tools)) = arrays.next() else {
        return Err(MclError::TypeMismatch);
    };
    let (tx, rx) = oneshot::channel();
    let pending = pending(world, entity, request);
    world.send_event(InferenceRequest {
        id: pending.operation_id.clone(),
        agent: entity,
        messages,
        tools,
        turn: turn.clone(),
        cancel: pending.cancel.clone(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    wait_result(world, request.clone(), pending, turn, async move {
        rx.await
            .map_err(|e| MclError::ExecutionFailed(e.to_string()))?
            .map(MclValue::Inference)
            .map_err(|e| {
                if e.kind == margatroid_types::inference_events::InferenceErrorKind::Cancelled {
                    MclError::Cancelled
                } else {
                    MclError::ExecutionFailed(e.to_string())
                }
            })
    });
    Ok(None)
}

fn visible_aliases(
    visible: &[CallableResource],
) -> Result<BTreeMap<String, CallableResource>, MclError> {
    let mut aliases = BTreeMap::new();
    for resource in visible {
        if aliases
            .insert(resource.alias.clone(), resource.clone())
            .is_some()
        {
            return Err(MclError::AliasConflict);
        }
    }
    Ok(aliases)
}

fn failed_call(call: &ToolCall, resource_id: Option<ResourceID>, message: String) -> AgentMessage {
    AgentMessage::ToolCall {
        resource_id,
        tool_call_id: call.id.clone(),
        tool_name: call.tool_name.clone(),
        content: message,
        failed: true,
    }
}

fn begin_tools(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    arrays: Vec<BlockInner>,
) -> Result<Option<MclValue>, MclError> {
    let mut arrays = arrays.into_iter();
    let Some(BlockInner::Message(messages)) = arrays.next() else {
        return Err(MclError::TypeMismatch);
    };
    let Some(BlockInner::Tool(visible)) = arrays.next() else {
        return Err(MclError::TypeMismatch);
    };
    let [AgentMessage::Assistant {
        tool_calls: calls, ..
    }] = messages.as_slice()
    else {
        return Err(MclError::ToolCallInvalid);
    };
    let calls = calls.clone();
    let aliases = visible_aliases(&visible)?;
    if calls
        .iter()
        .all(|call| !aliases.contains_key(&call.tool_name))
    {
        let messages = calls
            .iter()
            .map(|call| failed_call(call, None, "tool not visible".into()))
            .collect::<Vec<_>>();
        let history_error = append_messages(world, entity, request, turn, &messages).err();
        let mut reply = MclCommandResult::from_request(
            request,
            Ok(MclValue::Array(BlockInner::Message(messages))),
        );
        reply.history_error = history_error;
        crate::runtime::publish_result(world, reply);
        finish_head(world, entity);
        return Ok(None);
    }
    let pending = pending(world, entity, request);
    let mut slots = Vec::new();
    for (index, call) in calls.into_iter().enumerate() {
        let resource = aliases.get(&call.tool_name).cloned();
        let receiver = resource.as_ref().map(|resource| {
            let (tx, rx) = oneshot::channel();
            world.send_event(ToolExecuteRequest {
                id: format!("{}:{index}", pending.operation_id),
                agent: entity,
                resource: resource.clone(),
                call: call.clone(),
                turn: turn.clone(),
                cancel: pending.cancel.child_token(),
                reply: Arc::new(Mutex::new(Some(tx))),
            });
            rx
        });
        slots.push((call, resource, receiver));
    }
    wait_result(world, request.clone(), pending, turn, async move {
        let mut messages = Vec::new();
        for (call, resource, receiver) in slots {
            messages.push(match receiver {
                None => failed_call(
                    &call,
                    resource.map(|r| r.resource_id),
                    "tool not visible".into(),
                ),
                Some(rx) => match rx.await {
                    Ok(Ok(response)) => response.message,
                    Ok(Err(error)) => {
                        failed_call(&call, resource.map(|r| r.resource_id), error.to_string())
                    }
                    Err(error) => {
                        failed_call(&call, resource.map(|r| r.resource_id), error.to_string())
                    }
                },
            });
        }
        Ok(MclValue::Array(BlockInner::Message(messages)))
    });
    Ok(None)
}

fn begin_import(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    declared_kind: InnerType,
    resource_id: ResourceID,
    alias: String,
) -> Result<Option<MclValue>, MclError> {
    let agent = world.get_component::<Agent>(entity).unwrap();
    if !agent.info.image_dependencies.contains(&resource_id) {
        return Err(MclError::ImportNotAllowed);
    }
    parser::validate_import_kind(declared_kind, &resource_id)?;
    world
        .agent_validate_import(&agent.resourceid, &alias, declared_kind, &resource_id)
        .map_err(|e| match e.kind {
            margatroid_types::agent_events::AgentErrorKind::AliasConflict => {
                MclError::AliasConflict
            }
            _ => MclError::ImportNotAllowed,
        })?;
    let kind = resource_id.resource_type().to_owned();
    if !matches!(
        kind.as_str(),
        "tool" | "skill" | "hook" | "prompt" | "sandbox"
    ) {
        return Err(MclError::InvalidResourceId);
    }
    let activity = Arc::new(
        agent
            .execution
            .register(ExecutionActivityKind::Other)
            .map_err(|e| MclError::ExecutionFailed(e.to_string()))?,
    );
    let (tx, rx) = oneshot::channel();
    let pending = pending(world, entity, request);
    let reply = Arc::new(Mutex::new(Some(tx)));
    macro_rules! load {
        ($ty:ident) => {
            world.send_event($ty {
                id: pending.operation_id.clone(),
                target: entity,
                resource_id: resource_id.clone(),
                alias: alias.clone(),
                import_kind: Some(declared_kind),
                cancel: pending.cancel.child_token(),
                reply: reply.clone(),
                activity: Some(activity.clone()),
            })
        };
    }
    match kind.as_str() {
        "tool" => load!(LoadToolResource),
        "skill" => load!(LoadSkillResource),
        "hook" => load!(LoadHookResource),
        "prompt" => load!(LoadPromptResource),
        "sandbox" => load!(LoadSandboxResource),
        _ => unreachable!(),
    }
    wait_result(world, request.clone(), pending, turn, async move {
        rx.await
            .map_err(|e| MclError::ExecutionFailed(e.to_string()))?
            .map_err(|e| {
                if e.kind == ResourceLoadErrorKind::Cancelled {
                    MclError::Cancelled
                } else {
                    MclError::ExecutionFailed(e.to_string())
                }
            })?;
        Ok(MclValue::ResourceImport {
            kind: declared_kind,
            resource_id,
            alias,
        })
    });
    Ok(None)
}
