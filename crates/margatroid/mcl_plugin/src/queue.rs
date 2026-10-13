use crate::parser::{self, ParsedOperation};
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{Entity, World};
use margatroid_types::agent_components::{Agent, AgentState, WorldAgentExt};
use margatroid_types::agent_components::{ImportBindings, ImportedValue};
use margatroid_types::block_storage::*;
use margatroid_types::block_types::{AgentBlock, BlockInner, InnerType};
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

fn contains_system(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::System { .. } => true,
        AgentMessage::Inject { messages } => messages.iter().any(contains_system),
        _ => false,
    }
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
    let result = match output.result {
        Ok(MclAsyncPhase::Completed(value)) => Ok(Some(value)),
        Ok(MclAsyncPhase::PreparedEffect { name, arrays }) => {
            if output.pending.cancel.is_cancelled()
                || crate::runtime::driver_command_cancelled(world, &output.request)
                || world.get_component::<Agent>(entity).is_some_and(stopped)
            {
                Err(MclError::Cancelled)
            } else {
                execute_effect(world, entity, &output.request, output.turn, &name, arrays)
            }
        }
        Ok(MclAsyncPhase::PreparedImport {
            kind,
            resource_id,
            alias,
        }) => {
            if output.pending.cancel.is_cancelled()
                || crate::runtime::driver_command_cancelled(world, &output.request)
                || world.get_component::<Agent>(entity).is_some_and(stopped)
            {
                Err(MclError::Cancelled)
            } else {
                begin_import(
                    world,
                    entity,
                    &output.request,
                    output.turn,
                    kind,
                    resource_id,
                    alias,
                )
            }
        }
        Err(MclError::StorageCleanupFailed(message)) => {
            let agent = world.get_component_mut::<Agent>(entity).unwrap();
            agent.execution.request_close();
            agent.mcl.accepting = false;
            agent.mcl.drained.mark_cleanup_failed(
                margatroid_types::execution_components::ExecutionError::CleanupFailed {
                    activity_id: 0,
                    message: message.clone(),
                },
            );
            Err(MclError::StorageCleanupFailed(message))
        }
        Err(error) => Err(error),
    };
    if matches!(result, Ok(None)) {
        return;
    }
    let reply = MclCommandResult::from_request(&output.request, result.map(|value| value.unwrap()));
    crate::runtime::publish_result(world, reply);
    finish_head(world, entity);
}

fn finish_head(world: &mut World, entity: Entity) {
    if let Some(agent) = world.get_component_mut::<Agent>(entity) {
        agent.mcl.commands.pop_front();
        agent.mcl.pending = None;
        agent.mcl.drained.set_pending_cancel(None);
        if agent.mcl.commands.is_empty() && agent.mcl.cancel_audits == 0 {
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
    let mut unstarted = Vec::new();
    for request in cancelled {
        let agent = world.get_component_mut::<Agent>(entity).unwrap();
        if let Some(pending) = agent
            .mcl
            .pending
            .as_ref()
            .filter(|pending| pending.command_id == request.id)
        {
            pending.cancel.cancel();
        } else {
            agent.mcl.commands.retain(|entry| entry != &request);
            unstarted.push(request);
        }
    }
    audit_cancelled(world, entity, unstarted, turn);
    let agent = world.get_component_mut::<Agent>(entity).unwrap();
    if agent.mcl.commands.is_empty() && agent.mcl.pending.is_none() && agent.mcl.cancel_audits == 0
    {
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
        audit_cancelled(world, entity, cancelled.into_iter().collect(), turn);
        let agent = world.get_component_mut::<Agent>(entity).unwrap();
        if agent.mcl.commands.is_empty()
            && agent.mcl.pending.is_none()
            && agent.mcl.cancel_audits == 0
        {
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
        let result = begin_database(world, entity, &request, turn, parsed);
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

fn storage_error(error: BlockStorageError) -> MclError {
    match error {
        BlockStorageError::Cancelled => MclError::Cancelled,
        BlockStorageError::CommitUnknown(message) => MclError::StorageCommitUnknown(message),
        BlockStorageError::CleanupFailed(message) => MclError::StorageCleanupFailed(message),
        BlockStorageError::TypeMismatch => MclError::TypeMismatch,
        BlockStorageError::InvalidRange => MclError::InvalidRange,
        BlockStorageError::BlockMissing(_) | BlockStorageError::FieldMissing(_) => {
            MclError::SourceReadFailed
        }
        error => MclError::ExecutionFailed(error.to_string()),
    }
}

fn history_error(error: HistoryError) -> MclError {
    match error {
        HistoryError::CleanupFailed(message) => MclError::StorageCleanupFailed(message),
        HistoryError::CommitUnknown(message) => MclError::StorageCommitUnknown(message),
        error => MclError::HistoryWriteFailed(error.to_string()),
    }
}

fn stored_alias(bindings: &ImportBindings, alias: &str) -> Result<StoredBlockInner, MclError> {
    match bindings.get(alias).ok_or(MclError::AliasMissing)? {
        ImportedValue::Message { messages, .. } => Ok(StoredBlockInner::Message(messages.to_vec())),
        ImportedValue::Tool(value) => Ok(StoredBlockInner::Tool(vec![value.resource_id.clone()])),
        ImportedValue::Sandbox(value) => {
            Ok(StoredBlockInner::Sandbox(vec![value.resource_id.clone()]))
        }
    }
}

fn selector(value: &parser::Selector) -> BlockSelector {
    let endpoint = |value: parser::Endpoint| match value {
        parser::Endpoint::Open => None,
        parser::Endpoint::Value(value) => Some(value),
    };
    BlockSelector {
        path: value.path.clone(),
        range: match value.range {
            parser::Range::All => BlockRange::All,
            parser::Range::Empty => BlockRange::Empty,
            parser::Range::Single(value) => BlockRange::Single(value),
            parser::Range::Range(a, b) => BlockRange::Range(endpoint(a), endpoint(b)),
        },
    }
}

fn expression(
    bindings: &ImportBindings,
    value: &parser::ArrayExpression,
) -> Result<BlockExpression, MclError> {
    value
        .sources
        .iter()
        .map(|source| match source {
            parser::ArraySource::Alias(alias) => {
                stored_alias(bindings, alias).map(BlockSource::Values)
            }
            parser::ArraySource::Selector(value) => Ok(BlockSource::Selector(selector(value))),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(BlockExpression::Sources)
}

fn parameter(
    bindings: &ImportBindings,
    value: &parser::ArrayParameter,
    arguments: &[serde_json::Value],
    kind: Option<InnerType>,
) -> Result<BlockExpression, MclError> {
    match value {
        parser::ArrayParameter::Expression(value) => expression(bindings, value),
        parser::ArrayParameter::Empty => Ok(BlockExpression::Empty),
        parser::ArrayParameter::Manual { index } => {
            let value = arguments.get(*index).ok_or(MclError::BindingMissing)?;
            let values = value.as_array().ok_or(MclError::TypeMismatch)?;
            if matches!(kind, Some(InnerType::Tool | InnerType::Sandbox))
                || (kind.is_none() && values.first().is_some_and(serde_json::Value::is_string))
            {
                let sources = values
                    .iter()
                    .map(|value| {
                        let alias = value.as_str().ok_or(MclError::TypeMismatch)?;
                        parser::validate_alias(alias)?;
                        stored_alias(bindings, alias).map(BlockSource::Values)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if sources.is_empty() {
                    return Ok(BlockExpression::Sources(vec![BlockSource::Values(
                        StoredBlockInner::empty(kind.unwrap()),
                    )]));
                }
                Ok(BlockExpression::Sources(sources))
            } else if values.is_empty() && kind.is_none() {
                Ok(BlockExpression::Empty)
            } else {
                Ok(BlockExpression::Sources(vec![BlockSource::Values(
                    parser::decode_manual_array(value, InnerType::Message)?.into(),
                )]))
            }
        }
    }
}

fn materialize(bindings: &ImportBindings, value: StoredBlockInner) -> Result<BlockInner, MclError> {
    match value {
        StoredBlockInner::Message(values) => Ok(BlockInner::Message(values)),
        StoredBlockInner::Tool(ids) => ids
            .into_iter()
            .map(|id| match bindings.get_resource(&id, InnerType::Tool) {
                Some(ImportedValue::Tool(value)) => Ok(value.clone()),
                _ => Err(MclError::ResourceMissing),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(BlockInner::Tool),
        StoredBlockInner::Sandbox(ids) => ids
            .into_iter()
            .map(|id| match bindings.get_resource(&id, InnerType::Sandbox) {
                Some(ImportedValue::Sandbox(value)) => Ok(value.clone()),
                _ => Err(MclError::ResourceMissing),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(BlockInner::Sandbox),
    }
}

fn begin_database(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    operation: Result<ParsedOperation, MclError>,
) -> Result<Option<MclValue>, MclError> {
    let agent = world
        .get_component::<Agent>(entity)
        .ok_or(MclError::AgentMissing)?;
    let bindings = agent.import_bindings.clone();
    let context = agent
        .lua
        .instance
        .clone()
        .map(|instance| BlockStorageContext {
            agent: agent.resourceid.clone(),
            instance,
        });
    let storage = world
        .agent_block_storage(&agent.resourceid)
        .map_err(|e| MclError::ExecutionFailed(e.to_string()))?;
    let history = agent.history.clone();
    let activity = agent
        .execution
        .register(ExecutionActivityKind::Other)
        .map_err(|e| MclError::ExecutionFailed(e.to_string()))?;
    activity
        .mark_started()
        .map_err(|e| MclError::ExecutionFailed(e.to_string()))?;
    let mut pending = pending(world, entity, request);
    pending.operation_id = format!("database:{}", pending.operation_id);
    world
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .mcl
        .pending = Some(pending.clone());
    let events = world.event_sender();
    let request = request.clone();
    let cancel = pending.cancel.clone();
    world.spawn_async_service(async move {
        let is_append = matches!(&operation, Ok(ParsedOperation::Effect {name,..}) if name == "history_append");
        let audit_history = history.clone();
        let result = async {
            let audit = history_record(entity, &request, turn.clone(), "mcl", "command", serde_json::json!({"text":request.command.text,"arguments":request.command.arguments,"cancelled":false}));
            if !is_append {
                let history = history.clone();
                tokio::task::spawn_blocking(move || history.append(audit)).await
                    .map_err(|error| MclError::HistoryWriteFailed(error.to_string()))?
                    .map_err(history_error)?;
            }
            if cancel.is_cancelled() { return Err(MclError::Cancelled); }
            let (db_operation, effect) = match operation? {
                ParsedOperation::Create {block_id,fields,temporary} => (Some(BlockStorageOperation::Create(BlockSchema {name:block_id,fields:fields.into_iter().map(|field|(field.inner_id,field.kind)).collect(),temporary})), None),
                ParsedOperation::Get {expression:value} => (Some(BlockStorageOperation::Get {expressions:vec![expression(&bindings,&value)?]}),None),
                ParsedOperation::Inject {source,target} => (Some(BlockStorageOperation::Inject {source:parameter(&bindings,&source,&request.command.arguments,None)?,target:selector(&target)}),None),
                ParsedOperation::Effect {name,parameters} => {
                    let kinds = parser::effect_types(&name)?;
                    let expressions = parameters.iter().zip(kinds).map(|(value,kind)| parameter(&bindings,value,&request.command.arguments,Some(*kind))).collect::<Result<Vec<_>,_>>()?;
                    if expressions.iter().any(|expression| matches!(expression, BlockExpression::Sources(sources) if sources.iter().any(|source| matches!(source,BlockSource::Selector(_))))) {
                        (Some(BlockStorageOperation::Get {expressions}),Some((name,parameters)))
                    } else {
                        let arrays = parser::evaluate_effect_arguments_with(&|alias| materialize(&bindings,stored_alias(&bindings,alias)?),&AgentBlock::default(),&name,&parameters,&request.command.arguments)?;
                        return finish_preparation(history, entity, &request, turn.clone(), name, arrays).await;
                    }
                }
                ParsedOperation::Import {kind,resource_id,alias} => return Ok(MclAsyncPhase::PreparedImport {kind,resource_id,alias}),
            };
            let output = storage.execute(BlockStorageRequest {context:context.ok_or(MclError::InvalidRequest)?,operation:db_operation.unwrap(),cancel:cancel.clone()}).await.map_err(storage_error)?;
            match output {
                StoredBlockResult::BlockOpened(value) => Ok(MclAsyncPhase::Completed(MclValue::BlockOpened {block_id:value.block_id,created:value.created,temporary:value.temporary})),
                StoredBlockResult::Unit => Ok(MclAsyncPhase::Completed(MclValue::Unit)),
                StoredBlockResult::Arrays(values) => {
                    let arrays = values.into_iter().map(|value| materialize(&bindings,value)).collect::<Result<Vec<_>,_>>()?;
                    if let Some((name,_)) = effect {
                        validate_prepared_effect(&name,&arrays)?;
                        finish_preparation(history,entity,&request,turn.clone(),name,arrays).await
                    } else {
                        Ok(MclAsyncPhase::Completed(MclValue::Array(arrays.into_iter().next().ok_or(MclError::SourceReadFailed)?)))
                    }
                }
            }
        }.await;
        let result = if is_append && result.as_ref().is_err_and(|error| !matches!(error, MclError::HistoryWriteFailed(_) | MclError::StorageCleanupFailed(_) | MclError::StorageCommitUnknown(_) | MclError::Cancelled)) {
            let audit = history_record(entity, &request, turn.clone(), "mcl", "command", serde_json::json!({"text":request.command.text,"arguments":request.command.arguments,"cancelled":false}));
            match tokio::task::spawn_blocking(move || audit_history.append(audit)).await {
                Ok(Ok(())) => result,
                Ok(Err(error)) => Err(history_error(error)),
                Err(error) => Err(MclError::HistoryWriteFailed(error.to_string())),
            }
        } else { result };
        if let Err(MclError::StorageCleanupFailed(message)) = &result {
            activity.mark_cleanup_failed(margatroid_types::execution_components::ExecutionError::CleanupFailed {activity_id:activity.id,message:message.clone()});
        } else {
            let _ = activity.mark_completed();
        }
        events.send_event(MclAsyncOutput {request,pending,turn,result});
    });
    Ok(None)
}

fn validate_prepared_effect(name: &str, arrays: &[BlockInner]) -> Result<(), MclError> {
    let expected = parser::effect_types(name)?;
    if arrays.len() != expected.len()
        || arrays
            .iter()
            .zip(expected)
            .any(|(array, kind)| array.inner_type() != *kind)
    {
        return Err(MclError::TypeMismatch);
    }
    if name == "tool_call" {
        parser::validate_tool_parameters(arrays)?;
    }
    if name == "sandbox_use" && arrays[0].len() > 1 {
        return Err(MclError::EffectInvalid);
    }
    Ok(())
}

async fn finish_preparation(
    history: margatroid_types::history_components::AgentHistoryHandle,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    name: String,
    arrays: Vec<BlockInner>,
) -> Result<MclAsyncPhase, MclError> {
    if name == "history_append" {
        let BlockInner::Message(messages) = &arrays[0] else {
            return Err(MclError::TypeMismatch);
        };
        if messages.iter().any(contains_system) {
            return Err(MclError::TypeMismatch);
        };
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
        tokio::task::spawn_blocking(move || history.append_batch(records))
            .await
            .map_err(|error| MclError::HistoryWriteFailed(error.to_string()))?
            .map_err(history_error)?;
        Ok(MclAsyncPhase::Completed(MclValue::Unit))
    } else {
        Ok(MclAsyncPhase::PreparedEffect { name, arrays })
    }
}

fn execute_effect(
    world: &mut World,
    entity: Entity,
    request: &MclCommand,
    turn: Option<String>,
    name: &str,
    arrays: Vec<BlockInner>,
) -> Result<Option<MclValue>, MclError> {
    let agent_id = world
        .get_component::<Agent>(entity)
        .ok_or(MclError::AgentMissing)?
        .resourceid
        .clone();
    match name {
        "inference" => begin_inference(world, entity, request, turn, arrays),
        "tool_call" => begin_tools(world, entity, request, turn, arrays),
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
        "finish_turn" => {
            world.get_component_mut::<Agent>(entity).unwrap().turn = None;
            Ok(Some(MclValue::Unit))
        }
        _ => Err(MclError::EffectInvalid),
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
        cancel: agent
            .mcl
            .pending
            .as_ref()
            .filter(|pending| pending.command_id == request.id)
            .map(|pending| pending.cancel.clone())
            .unwrap_or_else(|| agent.execution.child_cancel()),
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
            result: result.map(MclAsyncPhase::Completed),
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
        let reply = MclCommandResult::from_request(
            request,
            Ok(MclValue::Array(BlockInner::Message(messages))),
        );
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

fn audit_cancelled(
    world: &mut World,
    entity: Entity,
    requests: Vec<MclCommand>,
    turn: Option<String>,
) {
    if requests.is_empty() {
        return;
    }
    let agent = world.get_component_mut::<Agent>(entity).unwrap();
    agent.mcl.cancel_audits += 1;
    let history = agent.history.clone();
    let events = world.event_sender();
    world.spawn_async_service(async move {
        let records = requests.iter().map(|request| history_record(entity,request,turn.clone(),"mcl","command",serde_json::json!({"text":request.command.text,"arguments":request.command.arguments,"cancelled":true}))).collect();
        let error = match tokio::task::spawn_blocking(move || history.append_batch(records)).await {
            Ok(result) => result.err(),
            Err(error) => Some(HistoryError::WriteFailed(error.to_string())),
        };
        let results = requests.iter().map(|request| {
            let mut result = MclCommandResult::from_request(request,Err(MclError::Cancelled));
            result.history_error = error.clone();
            result
        }).collect();
        events.send_event(MclCancelledOutput {entity,results});
    });
}

pub(crate) fn complete_cancelled(world: &mut World, output: MclCancelledOutput) {
    let cleanup_failed = output
        .results
        .iter()
        .any(|result| matches!(result.history_error, Some(HistoryError::CleanupFailed(_))));
    for result in output.results {
        crate::runtime::publish_result(world, result);
    }
    if let Some(agent) = world.get_component_mut::<Agent>(output.entity) {
        agent.mcl.cancel_audits = agent.mcl.cancel_audits.saturating_sub(1);
        if cleanup_failed {
            agent.execution.request_close();
            agent.mcl.accepting = false;
            agent.mcl.drained.mark_cleanup_failed(
                margatroid_types::execution_components::ExecutionError::CleanupFailed {
                    activity_id: 0,
                    message: "cancelled command audit cleanup failed".into(),
                },
            );
        }
        if agent.mcl.commands.is_empty()
            && agent.mcl.pending.is_none()
            && agent.mcl.cancel_audits == 0
        {
            agent.mcl.drained.mark_drained();
        }
    }
}
