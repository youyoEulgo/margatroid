use std::collections::{HashMap, HashSet};

use agent_plugin::{Agent, AgentInferencePending, AgentInitializationCompleted};
use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Plugin, Resource, World};
use lua_runtime_plugin::{
    CancellationToken, HostFuture, LuaBindingValue, LuaEnvironment, LuaEnvironmentContext,
    LuaEnvironmentProvider, LuaGlobalBinding, LuaHostFunction, LuaRuntimeError, LuaRuntimeHandle,
    LuaValue, LuaVmMessageReceived,
};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, AgentRealtimeContextReadCompleted,
    AgentRealtimeContextReadRequested, CapturedInferenceRequest, CapturedInferenceResponse,
    InferenceRequestEvent, Message, ResourceId, ToolCall, ToolCallEvent,
};
use resource_id_plugin::ResourceIdPluginInstalled;
use resource_id_plugin::WorldResourceIdExt;
use std::sync::Arc;
use tool_plugin::ToolPluginInstalled;
use tool_plugin::{
    register_agent_resource, resolve_agent_tool_definitions, AgentResourceRegisterRequest,
    AgentResourceRegisterResponse, ResourceContent, ResourceMapEntry,
};

use crate::{
    execute_direct_operation, parse_operation, MclCommandRequest, MclDomainRequest,
    MclDomainResponse, MclError, MclOperation,
};

#[derive(Default)]
pub struct PendingMclImports {
    pub imports: HashMap<String, crate::MclImportState>,
}
impl Resource for PendingMclImports {}
#[derive(Default)]
pub struct PendingMclEffects {
    pub effects: HashMap<String, crate::MclEffectState>,
    pub failures: HashMap<(core_plugin::Entity, String), MclError>,
}
impl Resource for PendingMclEffects {}

pub struct MclPlugin {
    schedule: String,
}
impl MclPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
    pub fn open(_root: impl Into<std::path::PathBuf>) -> Result<Self, MclError> {
        Ok(Self::new())
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
impl Default for MclPlugin {
    fn default() -> Self {
        Self::new()
    }
}
pub struct MclPluginInstalled;
impl Resource for MclPluginInstalled {}

impl Plugin for MclPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("MclPlugin requires RuntimePlugin");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("MclPlugin schedule does not exist");
        }
        if !app.world().contains_resource::<LuaRuntimeHandle>()
            || !app.world().contains_resource::<ResourceIdPluginInstalled>()
            || !app.world().contains_resource::<ToolPluginInstalled>()
        {
            panic!("MclPlugin dependency is missing");
        }
        app.world_mut().insert_resource(MclPluginInstalled);
        app.world_mut()
            .insert_resource(PendingMclImports::default());
        app.world_mut()
            .insert_resource(PendingMclEffects::default());
        let events = app.world().event_sender();
        app.world()
            .get_resource::<LuaRuntimeHandle>()
            .expect("MclPlugin requires LuaRuntimePlugin")
            .register_provider(Box::new(MclEnvironmentProvider { events }))
            .expect("mcl provider registration failed");
        app.add_system(&self.schedule, mcl_command_request_system)
            .add_system(&self.schedule, mcl_domain_system)
            .add_system(&self.schedule, mcl_import_response_system)
            .add_system(&self.schedule, mcl_effect_response_system)
            .add_system(&self.schedule, mcl_command_reply_system);
    }
}

pub(crate) struct MclEnvironmentProvider {
    events: RuntimeEventSender,
}
impl LuaEnvironmentProvider for MclEnvironmentProvider {
    fn name(&self) -> &str {
        "mcl"
    }
    fn provide(&self, _context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError> {
        Ok(LuaEnvironment {
            globals: vec![LuaGlobalBinding {
                name: "mcl".into(),
                binding: LuaBindingValue::Function(Arc::new(MclHostFunction {
                    events: self.events.clone(),
                })),
            }],
            modules: Vec::new(),
        })
    }
}

struct MclHostFunction {
    events: RuntimeEventSender,
}
impl LuaHostFunction for MclHostFunction {
    fn call(
        &self,
        arguments: LuaValue,
        _context: LuaEnvironmentContext,
        cancel: CancellationToken,
    ) -> HostFuture {
        let events = self.events.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(LuaRuntimeError::Cancelled);
            }
            let LuaValue::Array(mut values) = arguments else {
                return Err(LuaRuntimeError::InvalidRequest(
                    "mcl expects agent_id, command and binding".into(),
                ));
            };
            if values.len() < 2 || values.len() > 3 {
                return Err(LuaRuntimeError::InvalidRequest(
                    "mcl expects agent_id, command and binding".into(),
                ));
            }
            let binding = (values.len() == 3)
                .then(|| values.pop().unwrap())
                .filter(|value| !matches!(value, LuaValue::Nil))
                .map(lua_to_json_binding)
                .transpose()?;
            let command = match values.pop() {
                Some(LuaValue::String(value)) => value,
                _ => {
                    return Err(LuaRuntimeError::InvalidRequest(
                        "mcl command must be a string".into(),
                    ))
                }
            };
            let agent_id = match values.pop() {
                Some(LuaValue::String(value)) => value.parse().map_err(|_| {
                    LuaRuntimeError::InvalidRequest("mcl agent id is invalid".into())
                })?,
                _ => {
                    return Err(LuaRuntimeError::InvalidRequest(
                        "mcl agent id must be a string".into(),
                    ))
                }
            };
            let id = crate::MclCommandId::new(format!("lua-mcl-{}", next_mcl_call_id()))
                .map_err(|error| LuaRuntimeError::EnvironmentFailed(error.to_string()))?;
            let (sender, receiver) = tokio::sync::oneshot::channel();
            events.send_event(MclCommandRequest {
                id,
                agent_id,
                command,
                binding,
                reply: crate::MclCommandReply::new(sender),
            });
            let value = receiver
                .await
                .map_err(|_| LuaRuntimeError::RuntimeClosed)?
                .map_err(|error| LuaRuntimeError::EnvironmentFailed(error.to_string()))?;
            command_value_to_lua(value)
        })
    }
}

fn next_mcl_call_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn command_value_to_lua(value: crate::MclCommandValue) -> Result<LuaValue, LuaRuntimeError> {
    match value {
        crate::MclDomainValue::Unit => Ok(LuaValue::Nil),
        crate::MclDomainValue::Text(value) => Ok(LuaValue::String(value)),
        crate::MclDomainValue::Inner(value) => match value { margatroid_types::BlockInner::Message(values) => { let values = values.into_iter().map(mcl_message_to_json).collect::<Result<Vec<_>, _>>()?; json_to_lua(serde_json::to_value(values).map_err(|error| LuaRuntimeError::EnvironmentFailed(error.to_string()))?) } margatroid_types::BlockInner::ToolCall(values) => json_to_lua(serde_json::to_value(values).map_err(|error| LuaRuntimeError::EnvironmentFailed(error.to_string()))?), margatroid_types::BlockInner::ResourceId(values) => Ok(LuaValue::Array(values.into_iter().map(|value| LuaValue::String(value.to_string())).collect())) },
        crate::MclDomainValue::Message(value) => json_to_lua(mcl_message_to_json(value)?),
        crate::MclDomainValue::Paths(values) => json_to_lua(serde_json::to_value(values.into_iter().map(|value| serde_json::json!({"block_id": value.block_id, "inner_id": value.inner_id})).collect::<Vec<_>>()).unwrap()),
        crate::MclDomainValue::ResourceImport(value) => json_to_lua(serde_json::json!({"resource_id": value.resource_id.to_string(), "alias": value.alias, "available": value.available, "error": value.error})),
    }
}

pub fn command_value_to_json(value: crate::MclCommandValue) -> Result<serde_json::Value, MclError> {
    Ok(match value {
        crate::MclDomainValue::Unit => serde_json::Value::Null,
        crate::MclDomainValue::Text(value) => serde_json::Value::String(value),
        crate::MclDomainValue::Inner(value) => match value {
            margatroid_types::BlockInner::Message(values) => serde_json::Value::Array(
                values
                    .into_iter()
                    .map(mcl_message_to_json)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| MclError::TypeMismatch)?,
            ),
            margatroid_types::BlockInner::ToolCall(values) => {
                serde_json::to_value(values).map_err(|_| MclError::TypeMismatch)?
            }
            margatroid_types::BlockInner::ResourceId(values) => serde_json::Value::Array(
                values
                    .into_iter()
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .collect(),
            ),
        },
        crate::MclDomainValue::Message(value) => {
            mcl_message_to_json(value).map_err(|_| MclError::TypeMismatch)?
        }
        crate::MclDomainValue::Paths(values) => serde_json::to_value(
            values
                .into_iter()
                .map(|value| serde_json::json!({"block_id": value.block_id, "inner_id": value.inner_id}))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| MclError::TypeMismatch)?,
        crate::MclDomainValue::ResourceImport(value) => serde_json::json!({
            "resource_id": value.resource_id.to_string(),
            "alias": value.alias,
            "available": value.available,
            "error": value.error,
        }),
    })
}

fn mcl_message_to_json(
    message: margatroid_types::MclMessage,
) -> Result<serde_json::Value, LuaRuntimeError> {
    let mut value = match &message.message {
        margatroid_types::Message::System { content } => serde_json::json!({
            "type": "system",
            "content": content,
        }),
        margatroid_types::Message::User { content } => serde_json::json!({
            "type": "user",
            "content": content,
        }),
        margatroid_types::Message::Assistant {
            reasoning,
            content,
            tool_calls,
        } => serde_json::json!({
            "type": "assistant",
            "reasoning": reasoning,
            "content": content,
            "tool_calls": tool_calls,
        }),
        margatroid_types::Message::Tool {
            resource_id,
            tool_call_id,
            content,
        } => serde_json::json!({
            "type": "tool",
            "resource_id": resource_id,
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    };
    if let Some(usage) = message.usage {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "usage".to_owned(),
                serde_json::to_value(usage)
                    .map_err(|error| LuaRuntimeError::EnvironmentFailed(error.to_string()))?,
            );
        }
    }
    Ok(value)
}

fn json_to_lua(value: serde_json::Value) -> Result<LuaValue, LuaRuntimeError> {
    Ok(match value {
        serde_json::Value::Null => LuaValue::Nil,
        serde_json::Value::Bool(value) => LuaValue::Boolean(value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(LuaValue::Integer)
            .or_else(|| value.as_f64().map(LuaValue::Number))
            .ok_or_else(|| {
                LuaRuntimeError::EnvironmentFailed("JSON number is unsupported".into())
            })?,
        serde_json::Value::String(value) => LuaValue::String(value),
        serde_json::Value::Array(values) => LuaValue::Array(
            values
                .into_iter()
                .map(json_to_lua)
                .collect::<Result<_, _>>()?,
        ),
        serde_json::Value::Object(values) => LuaValue::Object(
            values
                .into_iter()
                .map(|(key, value)| Ok((key, json_to_lua(value)?)))
                .collect::<Result<_, LuaRuntimeError>>()?,
        ),
    })
}
fn lua_to_json_binding(value: LuaValue) -> Result<serde_json::Value, LuaRuntimeError> {
    lua_to_json(value).map_err(|error| LuaRuntimeError::InvalidRequest(error.to_string()))
}

pub fn mcl_command_request_system(world: &mut World) {
    let requests = world
        .event_reader::<MclCommandRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if request.agent_id.resource_type() != "agent" {
            request.reply.send(Err(MclError::InvalidAgentId));
            continue;
        }
        let operation = match parse_operation(&request.command, request.binding.as_ref()) {
            Ok(operation) => operation,
            Err(error) => {
                request.reply.send(Err(error));
                continue;
            }
        };
        if matches!(
            operation,
            MclOperation::Import { .. } | MclOperation::Emit { .. }
        ) {
            world.send_event(MclDomainRequest {
                id: request.id,
                agent_id: request.agent_id,
                operation,
                reply: request.reply,
            });
        } else {
            request
                .reply
                .send(execute_direct_operation(world, &request, operation));
        }
    }
}

pub fn mcl_domain_system(world: &mut World) {
    let requests = world
        .event_reader::<MclDomainRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        if matches!(
            request.operation,
            MclOperation::Emit {
                effect: crate::MclEffectCommand::Start
            }
        ) {
            if let Err(error) = begin_start(world, request.clone()) {
                world.send_event(MclDomainResponse {
                    id: request.id,
                    agent_id: request.agent_id,
                    result: Err(error),
                    reply: request.reply,
                });
            }
            continue;
        }
        if let MclOperation::Import { resource_id, alias } = request.operation.clone() {
            if let Err(error) = begin_import(world, request.clone(), resource_id, alias) {
                world.send_event(MclDomainResponse {
                    id: request.id,
                    agent_id: request.agent_id,
                    result: Err(error),
                    reply: request.reply,
                });
            }
            continue;
        }
        if matches!(
            request.operation,
            MclOperation::Emit {
                effect: crate::MclEffectCommand::RealtimeLoad
            }
        ) {
            if let Err(error) = begin_realtime_load(world, request.clone()) {
                world.send_event(MclDomainResponse {
                    id: request.id,
                    agent_id: request.agent_id,
                    result: Err(error),
                    reply: request.reply,
                });
            }
            continue;
        }
        if let MclOperation::Emit {
            effect: crate::MclEffectCommand::CatchInference { ref_block_id },
        } = request.operation.clone()
        {
            if let Err(error) = begin_catch_inference(world, request.clone(), ref_block_id) {
                world.send_event(MclDomainResponse {
                    id: request.id,
                    agent_id: request.agent_id,
                    result: Err(error),
                    reply: request.reply,
                });
            }
            continue;
        }
        let result = (|| -> Result<crate::MclDomainValue, MclError> {
            match request.operation.clone() {
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::HistoryAppend { message },
                } => crate::history_append(world, &request.agent_id, message),
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::RealtimeSource { ref_block_id },
                } => crate::realtime_source(world, &request.agent_id, ref_block_id),
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::Start,
                } => Err(MclError::EffectInvalid),
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::Inference { ref_block_id },
                } => {
                    let agent = world
                        .entity_by_resource_id(&request.agent_id)
                        .map_err(|_| MclError::AgentMissing)?;
                    let turn_id = world
                        .get_component::<Agent>(agent)
                        .and_then(|value| value.turn.turn_id.clone())
                        .ok_or(MclError::TurnMissing)?;
                    let agent_state = world
                        .get_component::<Agent>(agent)
                        .ok_or(MclError::AgentMissing)?;
                    let block = agent_state
                        .mcl
                        .ref_blocks()
                        .blocks
                        .get(&ref_block_id)
                        .ok_or(MclError::RefBlockMissing {
                            assembly: "agent".into(),
                            block: ref_block_id.clone(),
                        })?;
                    let message_merges = block
                        .merges
                        .values()
                        .filter(|merge| matches!(merge, margatroid_types::RefMerge::Message(_)))
                        .count();
                    let resource_merges = block
                        .merges
                        .values()
                        .filter(|merge| matches!(merge, margatroid_types::RefMerge::ResourceId(_)))
                        .count();
                    let has_unsupported_merge = block
                        .merges
                        .values()
                        .any(|merge| matches!(merge, margatroid_types::RefMerge::ToolCall(_)));
                    if message_merges != 1 || resource_merges > 1 || has_unsupported_merge {
                        return Err(MclError::MessageSourceUnavailable);
                    }
                    let merge_id = block
                        .merges
                        .iter()
                        .find_map(|(id, merge)| {
                            matches!(merge, margatroid_types::RefMerge::Message(_))
                                .then_some(id.clone())
                        })
                        .ok_or(MclError::MessageSourceUnavailable)?;
                    let values = agent_state
                        .mcl
                        .select(&margatroid_types::BlockPath {
                            block_id: ref_block_id.clone(),
                            inner_id: merge_id,
                        })
                        .map_err(|_| MclError::MessageSourceUnavailable)?;
                    let margatroid_types::BlockInner::Message(messages) = values else {
                        return Err(MclError::TypeMismatch);
                    };
                    let resource_merge_id = block.merges.iter().find_map(|(id, merge)| {
                        matches!(merge, margatroid_types::RefMerge::ResourceId(_)).then_some(id)
                    });
                    let visible_resources = if let Some(merge_id) = resource_merge_id {
                        match agent_state
                            .mcl
                            .select(&margatroid_types::BlockPath {
                                block_id: ref_block_id.clone(),
                                inner_id: merge_id.clone(),
                            })
                            .map_err(|_| MclError::MessageSourceUnavailable)?
                        {
                            margatroid_types::BlockInner::ResourceId(values) => values,
                            _ => return Err(MclError::TypeMismatch),
                        }
                    } else {
                        Vec::new()
                    };
                    let tools = resolve_agent_tool_definitions(world, agent, &visible_resources)
                        .map_err(|_| MclError::ImportMissing)?;
                    if let Some(agent_state) = world.get_component_mut::<Agent>(agent) {
                        agent_state.inference.pending.insert(
                            (agent, turn_id.clone()),
                            AgentInferencePending {
                                id: turn_id.clone(),
                                tool_schema: tools.clone(),
                            },
                        );
                    }
                    world.send_event(InferenceRequestEvent {
                        id: turn_id,
                        agent,
                        agent_id: request.agent_id.clone(),
                        messages: messages.into_iter().map(|value| value.message).collect(),
                        tools,
                    });
                    Ok(crate::MclDomainValue::Unit)
                }
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::ToolCall { calls },
                } => {
                    let agent = world
                        .entity_by_resource_id(&request.agent_id)
                        .map_err(|_| MclError::AgentMissing)?;
                    let turn_id = world
                        .get_component::<Agent>(agent)
                        .and_then(|value| value.turn.turn_id.clone())
                        .ok_or(MclError::TurnMissing)?;
                    if calls.is_empty() || calls.iter().any(|call| call.id.is_empty()) {
                        return Err(MclError::ToolCallInvalid);
                    }
                    let allowed_tools = world
                        .get_component::<Agent>(agent)
                        .and_then(|agent_state| {
                            agent_state
                                .inference
                                .pending
                                .get(&(agent, turn_id.clone()))
                                .map(|pending| {
                                    pending
                                        .tool_schema
                                        .iter()
                                        .map(|tool| tool.name.clone())
                                        .collect::<HashSet<_>>()
                                })
                        })
                        .ok_or(MclError::ToolCallInvalid)?;
                    for call in calls {
                        if allowed_tools.contains(&call.tool_name) {
                            world.send_event(ToolCallEvent {
                                turn_id: turn_id.clone(),
                                agent,
                                call,
                            });
                        } else {
                            let resource_id = rejected_tool_resource(world, agent, &call);
                            world.send_event(AgentMessage {
                                id: turn_id.clone(),
                                agent,
                                message: Message::Tool {
                                    resource_id,
                                    tool_call_id: call.id,
                                    content: format!(
                                        "tool call rejected: `{}` is not visible for this turn",
                                        call.tool_name
                                    ),
                                },
                                usage: None,
                            });
                        }
                    }
                    Ok(crate::MclDomainValue::Unit)
                }
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::VisibilitySource { source },
                } => {
                    let agent = world
                        .entity_by_resource_id(&request.agent_id)
                        .map_err(|_| MclError::AgentMissing)?;
                    let state = world
                        .get_component_mut::<Agent>(agent)
                        .ok_or(MclError::AgentMissing)?;
                    let values = state
                        .mcl
                        .select(&source)
                        .map_err(|_| MclError::TypeMismatch)?;
                    let margatroid_types::BlockInner::ResourceId(values) = values else {
                        return Err(MclError::TypeMismatch);
                    };
                    state.resources.visible = values.into_iter().collect();
                    state.resources.visible_source = Some(source);
                    Ok(crate::MclDomainValue::Unit)
                }
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::DefaultVisibilitySource { source },
                } => {
                    let agent = world
                        .entity_by_resource_id(&request.agent_id)
                        .map_err(|_| MclError::AgentMissing)?;
                    let state = world
                        .get_component_mut::<Agent>(agent)
                        .ok_or(MclError::AgentMissing)?;
                    let values = state
                        .mcl
                        .select(&source)
                        .map_err(|_| MclError::TypeMismatch)?;
                    let margatroid_types::BlockInner::ResourceId(values) = values else {
                        return Err(MclError::TypeMismatch);
                    };
                    state.resources.default_visible = values.into_iter().collect();
                    state.resources.default_visible_source = Some(source);
                    Ok(crate::MclDomainValue::Unit)
                }
                MclOperation::Emit {
                    effect: crate::MclEffectCommand::Finish,
                } => {
                    let agent = world
                        .entity_by_resource_id(&request.agent_id)
                        .map_err(|_| MclError::AgentMissing)?;
                    let state = world
                        .get_component_mut::<Agent>(agent)
                        .ok_or(MclError::AgentMissing)?;
                    let turn = state.turn.turn_id.clone().ok_or(MclError::TurnMissing)?;
                    if state
                        .tools
                        .pending
                        .keys()
                        .any(|(_, pending_turn, _)| pending_turn == &turn)
                    {
                        return Err(MclError::EffectInvalid);
                    }
                    state
                        .turn
                        .finish(&turn)
                        .map_err(|_| MclError::TurnMismatch)?;
                    Ok(crate::MclDomainValue::Unit)
                }
                MclOperation::Emit { .. } => Ok(crate::MclDomainValue::Unit),
                MclOperation::Import { .. } => Err(MclError::EffectInvalid),
                _ => Err(MclError::EffectInvalid),
            }
        })();
        world.send_event(MclDomainResponse {
            id: request.id,
            agent_id: request.agent_id,
            result,
            reply: request.reply,
        });
    }
}

fn rejected_tool_resource(
    world: &World,
    agent: core_plugin::Entity,
    call: &ToolCall,
) -> ResourceId {
    world
        .get_component::<Agent>(agent)
        .and_then(|agent_state| {
            agent_state
                .resources
                .tool_by_name(&call.tool_name)
                .map(|entry| entry.resource_id.clone())
        })
        .or_else(|| call.tool_name.parse::<ResourceId>().ok())
        .unwrap_or_else(|| {
            ResourceId::new("tool", "builtin", "invalid", None::<String>)
                .expect("built-in invalid tool id must be valid")
        })
}

fn begin_start(world: &mut World, request: MclDomainRequest) -> Result<(), MclError> {
    let agent = world
        .entity_by_resource_id(&request.agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    if let Some(turn_id) = world
        .get_component::<Agent>(agent)
        .and_then(|value| value.turn.turn_id.clone())
    {
        if let Some(error) = world
            .get_resource_mut::<PendingMclEffects>()
            .and_then(|pending| pending.failures.remove(&(agent, turn_id.clone())))
        {
            if let Some(agent_state) = world.get_component_mut::<Agent>(agent) {
                agent_state.turn.abort();
            }
            return Err(error);
        }
    }
    let vm_id = world
        .get_component::<Agent>(agent)
        .ok_or(MclError::AgentMissing)
        .and_then(|value| {
            if !matches!(
                value.lifecycle,
                agent_plugin::AgentLifecycleState::Creating
                    | agent_plugin::AgentLifecycleState::Running
            ) {
                return Err(MclError::AgentRuntimeMissing);
            }
            value.lua.vm_id.ok_or(MclError::AgentRuntimeMissing)
        })?;
    let effect_id = format!("mcl-effect:{}", request.id.as_str());
    let state = crate::MclEffectState {
        command_id: request.id,
        agent_id: request.agent_id,
        agent,
        vm_id: Some(vm_id),
        kind: crate::MclPendingEffectKind::Start { vm_id },
        reply: request.reply,
    };
    let pending = world
        .get_resource_mut::<PendingMclEffects>()
        .ok_or(MclError::EffectAlreadyPending)?;
    if pending.effects.insert(effect_id.clone(), state).is_some() {
        return Err(MclError::EffectAlreadyPending);
    }
    let runtime = world
        .get_resource::<LuaRuntimeHandle>()
        .cloned()
        .ok_or(MclError::AgentRuntimeMissing)?;
    if runtime.receive_message(effect_id.clone(), vm_id).is_err() {
        world
            .get_resource_mut::<PendingMclEffects>()
            .map(|value| value.effects.remove(&effect_id));
        return Err(MclError::MailboxFailed);
    }
    let should_complete = world.get_component::<Agent>(agent).is_some_and(|value| {
        value.lifecycle == agent_plugin::AgentLifecycleState::Creating
            && !value.creation.initialization.complete
    });
    if should_complete {
        world.send_event(AgentInitializationCompleted { agent, vm_id });
    }
    Ok(())
}

fn begin_realtime_load(world: &mut World, request: MclDomainRequest) -> Result<(), MclError> {
    let agent = world
        .entity_by_resource_id(&request.agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let id = format!("mcl-effect:{}", request.id.as_str());
    let state = crate::MclEffectState {
        command_id: request.id,
        agent_id: request.agent_id,
        agent,
        vm_id: None,
        kind: crate::MclPendingEffectKind::RealtimeLoad,
        reply: request.reply,
    };
    let pending = world
        .get_resource_mut::<PendingMclEffects>()
        .ok_or(MclError::EffectAlreadyPending)?;
    if pending.effects.insert(id.clone(), state).is_some() {
        return Err(MclError::EffectAlreadyPending);
    }
    world.send_event(AgentRealtimeContextReadRequested { id, agent });
    Ok(())
}

fn begin_catch_inference(
    world: &mut World,
    request: MclDomainRequest,
    ref_block_id: String,
) -> Result<(), MclError> {
    let agent = world
        .entity_by_resource_id(&request.agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let _turn_id = world
        .get_component::<Agent>(agent)
        .and_then(|value| value.turn.turn_id.clone())
        .ok_or(MclError::TurnMissing)?;
    let agent_state = world
        .get_component::<Agent>(agent)
        .ok_or(MclError::AgentMissing)?;
    let block = agent_state
        .mcl
        .ref_blocks()
        .blocks
        .get(&ref_block_id)
        .ok_or(MclError::RefBlockMissing {
            assembly: "agent".into(),
            block: ref_block_id.clone(),
        })?;
    let message_merge = block
        .merges
        .values()
        .filter(|merge| matches!(merge, margatroid_types::RefMerge::Message(_)))
        .count();
    let forbidden_merge = block
        .merges
        .values()
        .any(|merge| !matches!(merge, margatroid_types::RefMerge::Message(_)));
    if message_merge != 1 || forbidden_merge {
        return Err(MclError::MessageSourceUnavailable);
    }
    let merge_id = block
        .merges
        .iter()
        .find_map(|(id, merge)| {
            matches!(merge, margatroid_types::RefMerge::Message(_)).then_some(id.clone())
        })
        .ok_or(MclError::MessageSourceUnavailable)?;
    let values = agent_state
        .mcl
        .select(&margatroid_types::BlockPath {
            block_id: ref_block_id,
            inner_id: merge_id,
        })
        .map_err(|_| MclError::MessageSourceUnavailable)?;
    let margatroid_types::BlockInner::Message(messages) = values else {
        return Err(MclError::TypeMismatch);
    };
    let effect_id = format!("mcl-effect:{}", request.id.as_str());
    let state = crate::MclEffectState {
        command_id: request.id,
        agent_id: request.agent_id.clone(),
        agent,
        vm_id: None,
        kind: crate::MclPendingEffectKind::CatchInference,
        reply: request.reply,
    };
    let pending = world
        .get_resource_mut::<PendingMclEffects>()
        .ok_or(MclError::EffectAlreadyPending)?;
    if pending.effects.insert(effect_id.clone(), state).is_some() {
        return Err(MclError::EffectAlreadyPending);
    }
    world.send_event(CapturedInferenceRequest {
        id: effect_id.clone(),
        agent,
        agent_id: request.agent_id,
        messages: messages.into_iter().map(|value| value.message).collect(),
    });
    Ok(())
}

pub fn mcl_effect_response_system(world: &mut World) {
    let responses = world
        .event_reader::<LuaVmMessageReceived>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in responses {
        let state = world
            .get_resource_mut::<PendingMclEffects>()
            .and_then(|pending| pending.effects.remove(&response.id));
        let Some(state) = state else {
            continue;
        };
        let is_start = matches!(state.kind, crate::MclPendingEffectKind::Start { .. });
        let result = if !is_start {
            Err(MclError::EffectResponseMismatch)
        } else if Some(response.vm_id) != state.vm_id {
            Err(MclError::EffectResponseMismatch)
        } else {
            response
                .result
                .map_err(|_| MclError::MailboxFailed)
                .and_then(parse_mailbox_message)
                .and_then(|envelope| {
                    let agent = world
                        .get_component_mut::<Agent>(state.agent)
                        .ok_or(MclError::AgentMissing)?;
                    match &envelope.message.message {
                        margatroid_types::Message::User { .. } => agent
                            .turn
                            .begin(envelope.turn_id.clone())
                            .map_err(|_| MclError::TurnMismatch)?,
                        margatroid_types::Message::Assistant { .. }
                        | margatroid_types::Message::Tool { .. }
                            if agent.turn.turn_id.as_deref() != Some(envelope.turn_id.as_str()) =>
                        {
                            return Err(MclError::TurnMismatch)
                        }
                        _ => {}
                    }
                    if let Some(usage) = envelope.message.usage.as_ref() {
                        agent.token_usage.add(usage);
                    }
                    Ok(crate::MclDomainValue::Message(envelope.message))
                })
        };
        world.send_event(MclDomainResponse {
            id: state.command_id,
            agent_id: state.agent_id,
            result,
            reply: state.reply,
        });
    }

    let realtime = world
        .event_reader::<AgentRealtimeContextReadCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in realtime {
        let state = world
            .get_resource_mut::<PendingMclEffects>()
            .and_then(|pending| pending.effects.remove(&response.id));
        let Some(state) = state else { continue };
        let result = match state.kind {
            crate::MclPendingEffectKind::RealtimeLoad => response
                .result
                .map(|messages| {
                    crate::MclDomainValue::Inner(margatroid_types::BlockInner::Message(messages))
                })
                .map_err(|_| MclError::RealtimeReadFailed),
            _ => Err(MclError::EffectResponseMismatch),
        };
        world.send_event(MclDomainResponse {
            id: state.command_id,
            agent_id: state.agent_id,
            result,
            reply: state.reply,
        });
    }

    let captured = world
        .event_reader::<CapturedInferenceResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in captured {
        let state = world
            .get_resource_mut::<PendingMclEffects>()
            .and_then(|pending| pending.effects.remove(&response.id));
        let Some(state) = state else { continue };
        let result = if state.agent != response.agent
            || !matches!(state.kind, crate::MclPendingEffectKind::CatchInference)
        {
            Err(MclError::EffectResponseMismatch)
        } else {
            response
                .result
                .map(|content| crate::MclDomainValue::Text(content))
                .map_err(|_| MclError::InferenceFailed)
        };
        world.send_event(MclDomainResponse {
            id: state.command_id,
            agent_id: state.agent_id,
            result,
            reply: state.reply,
        });
    }

    let failures = world
        .event_reader::<AgentFailure>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for failure in failures {
        let pending_key = world
            .get_resource::<PendingMclEffects>()
            .and_then(|pending| {
                pending.effects.iter().find_map(|(key, state)| {
                    (state.agent == failure.agent
                        && world
                            .get_component::<Agent>(state.agent)
                            .and_then(|agent| agent.turn.turn_id.as_deref())
                            == Some(failure.id.as_str())
                        && matches!(state.kind, crate::MclPendingEffectKind::Start { .. }))
                    .then(|| key.clone())
                })
            });
        if let Some(key) = pending_key {
            if let Some(state) = world
                .get_resource_mut::<PendingMclEffects>()
                .and_then(|pending| pending.effects.remove(&key))
            {
                let error = match failure.kind {
                    AgentFailureKind::Inference => MclError::InferenceFailed,
                    AgentFailureKind::Tool => MclError::ToolCallInvalid,
                    AgentFailureKind::Agent => MclError::EffectInvalid,
                };
                world.send_event(MclDomainResponse {
                    id: state.command_id,
                    agent_id: state.agent_id,
                    result: Err(error),
                    reply: state.reply,
                });
            }
        } else {
            world
                .get_resource_mut::<PendingMclEffects>()
                .map(|pending| {
                    pending
                        .failures
                        .insert((failure.agent, failure.id), MclError::InferenceFailed);
                });
        }
    }
}

fn parse_mailbox_message(
    value: LuaValue,
) -> Result<margatroid_types::AgentLuaMessageEnvelope, MclError> {
    let envelope: margatroid_types::AgentLuaMessageEnvelope =
        serde_json::from_value(lua_to_json(value)?).map_err(|_| MclError::MailboxFailed)?;
    Ok(envelope)
}

fn lua_to_json(value: LuaValue) -> Result<serde_json::Value, MclError> {
    Ok(match value {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(value) => serde_json::Value::Bool(value),
        LuaValue::Integer(value) => serde_json::Value::Number(value.into()),
        LuaValue::Number(value) => serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or(MclError::MailboxFailed)?,
        LuaValue::String(value) => serde_json::Value::String(value),
        LuaValue::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(lua_to_json)
                .collect::<Result<_, _>>()?,
        ),
        LuaValue::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| Ok((key, lua_to_json(value)?)))
                .collect::<Result<_, MclError>>()?,
        ),
    })
}

fn begin_import(
    world: &mut World,
    request: MclDomainRequest,
    resource_id: margatroid_types::ResourceId,
    alias: String,
) -> Result<(), MclError> {
    let agent = world
        .entity_by_resource_id(&request.agent_id)
        .map_err(|_| MclError::AgentMissing)?;
    let agent_info = world
        .get_component::<Agent>(agent)
        .ok_or(MclError::AgentMissing)?
        .info
        .clone();
    let is_soul_prompt = resource_id.resource_type() == "prompt"
        && resource_id.scope() == "system"
        && resource_id.name() == "soul";
    let is_compact_prompt = resource_id.resource_type() == "prompt"
        && resource_id.scope() == "user"
        && resource_id.name() == "compact";
    let soul_path = agent_info.image_root.join("SOUL.md");
    let compact_path = agent_info.image_root.join("COMPACT.md");
    let allowed = agent_info
        .image_dependencies
        .iter()
        .any(|dependency| dependency == &resource_id)
        || (is_soul_prompt && soul_path.is_file())
        || (is_compact_prompt && compact_path.is_file());
    if !allowed {
        return Err(MclError::ImportMissing);
    }
    let state = crate::MclImportState {
        command_id: request.id.clone(),
        agent_id: request.agent_id.clone(),
        agent,
        resource_id: resource_id.clone(),
        alias: alias.clone(),
        reply: request.reply.clone(),
    };
    let id = format!("mcl-import:{}", request.id.as_str());
    let prompt_content = if is_soul_prompt {
        let content = std::fs::read_to_string(&soul_path).map_err(|_| MclError::ImportMissing)?;
        if content.trim().is_empty() {
            return Err(MclError::ImportMissing);
        }
        Some(Arc::<str>::from(content))
    } else if is_compact_prompt {
        let content =
            std::fs::read_to_string(&compact_path).map_err(|_| MclError::ImportMissing)?;
        if content.trim().is_empty() {
            return Err(MclError::ImportMissing);
        }
        Some(Arc::<str>::from(content))
    } else {
        None
    };
    world
        .get_resource_mut::<PendingMclImports>()
        .ok_or(MclError::ImportFailed)?
        .imports
        .insert(id.clone(), state);
    if resource_id.resource_type() == "prompt" {
        let role = if resource_id.scope() == "system" {
            "system"
        } else {
            "user"
        };
        world.send_event(AgentResourceRegisterResponse {
            id,
            agent,
            resource_id: resource_id.clone(),
            alias: Some(alias.clone()),
            result: Ok(ResourceMapEntry {
                resource_id: resource_id.clone(),
                resource_name: alias.clone(),
                alias: Some(alias.clone()),
                tool_id: None,
                template: None,
                content: Some(ResourceContent::Prompt {
                    role: role.into(),
                    content: prompt_content.expect("prompt content was validated above"),
                }),
            }),
        });
        return Ok(());
    }
    world.send_event(AgentResourceRegisterRequest {
        id,
        agent,
        resource_id,
        alias: Some(alias),
    });
    Ok(())
}

pub fn mcl_import_response_system(world: &mut World) {
    let responses = world
        .event_reader::<AgentResourceRegisterResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in responses {
        let state = world
            .get_resource_mut::<PendingMclImports>()
            .and_then(|pending| pending.imports.remove(&response.id));
        let Some(state) = state else {
            continue;
        };
        let result = if response.agent != state.agent || response.resource_id != state.resource_id {
            Err(MclError::ImportResponseMismatch)
        } else {
            match response.result {
                Err(_) => Err(MclError::ImportFailed),
                Ok(mut entry) => {
                    entry.alias = Some(state.alias.clone());
                    entry.resource_name = state.alias.clone();
                    let alias_conflict = world
                        .get_component::<Agent>(state.agent)
                        .and_then(|agent| agent.resources.aliases.get(&state.alias))
                        .is_some_and(|resource| resource != &state.resource_id);
                    let is_prompt = entry.tool_id.is_none()
                        && entry.template.is_none()
                        && matches!(entry.content.as_ref(), Some(ResourceContent::Prompt { .. }));
                    if alias_conflict
                        || (!is_prompt
                            && register_agent_resource(world, state.agent, entry).is_err())
                    {
                        Err(MclError::ImportFailed)
                    } else if let Some(agent) = world.get_component_mut::<Agent>(state.agent) {
                        agent
                            .resources
                            .resources
                            .insert(state.resource_id.clone(), true);
                        agent
                            .resources
                            .aliases
                            .insert(state.alias.clone(), state.resource_id.clone());
                        Ok(crate::MclDomainValue::ResourceImport(
                            crate::ResourceImportReceipt {
                                resource_id: state.resource_id.clone(),
                                alias: state.alias.clone(),
                                available: true,
                                error: None,
                            },
                        ))
                    } else {
                        Err(MclError::ImportFailed)
                    }
                }
            }
        };
        world.send_event(MclDomainResponse {
            id: state.command_id,
            agent_id: state.agent_id,
            result,
            reply: state.reply,
        });
    }
}

pub fn mcl_command_reply_system(world: &mut World) {
    let responses = world
        .event_reader::<MclDomainResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in responses {
        response
            .reply
            .send(response.result.map(crate::domain_to_command));
    }
}
