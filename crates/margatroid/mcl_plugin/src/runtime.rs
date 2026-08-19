use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use app_runtime_plugin::WorldEventExt;
use core_plugin::{Entity, World};
use margatroid_types::{
    AgentRealtimeContextReadCompleted, AgentRealtimeContextReadRequested,
    AgentRealtimeContextWriteRequested, AgentRealtimeMessage, Message, ResourceId, ToolCall,
};

use crate::syntax::{
    MclBlockLifetime, MclPredicate, MclResourceExpression, MclStatement, MclViewKind,
};
use crate::{
    load_mcl_program, AgentMcl, AttachAgentMclRequest, AttachWorkflowMcl, DetachWorkflowMcl,
    MclBlock, MclCapabilityOwner, MclCapabilityStore, MclCommandReceived, MclCommandValue,
    MclContextItem, MclContextStore, MclDriverReady, MclEffect, MclEffectsProduced, MclError,
    MclErrorKind, MclHash, MclMessage, MclPluginInstalled, MclProgram, MclProgramKind,
    MclResourceAliasDeclared, MclRuntime, MclRuntimeEvent, MclRuntimeMessage,
    MclSnapshotProvenance, MclToolExchange, MclToolExchangeState, ModelRequestSnapshot,
    WorkflowInstanceId, WorkflowMclAttachFailed, WorkflowMclAttached, WorkflowMclDetachFailed,
    WorkflowMclDetached, WorkflowMclInstance,
};

#[derive(Default)]
pub(crate) struct MclDriverMailboxes {
    queues: HashMap<Entity, VecDeque<MclRuntimeMessage>>,
    waiting: HashMap<Entity, Sender<Result<MclCommandValue, MclError>>>,
    active_turns: HashMap<Entity, String>,
    realtime_reads: HashMap<String, Sender<Result<MclCommandValue, MclError>>>,
    drivers: HashMap<Entity, crate::MclDriverSource>,
    pending_tools: HashMap<Entity, Vec<ToolCall>>,
    states: HashMap<Entity, MclDriverContext>,
    ready: std::collections::HashSet<Entity>,
}

impl core_plugin::Resource for MclDriverMailboxes {}

#[derive(Clone, Default)]
struct MclDriverContext {
    message_block_created: bool,
    tool_block_created: bool,
    request_block_created: bool,
    compact_block_created: bool,
    system: Vec<MclMessage>,
    history_conversation: Vec<MclMessage>,
    recent_conversation: Vec<MclMessage>,
    compact: Vec<MclMessage>,
    compact_prompt: Option<String>,
    context_window_tokens: u64,
    tool_default: Vec<ResourceId>,
    tool_dynamic: Vec<ResourceId>,
    message_fields: HashMap<String, MclFieldType>,
    tool_fields: HashMap<String, MclFieldType>,
    imports: HashMap<String, ResourceId>,
    realtime_source: Option<String>,
    last_realtime_snapshot: Vec<AgentRealtimeMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MclFieldType {
    Message,
    ToolCall,
    Tool,
}

fn parse_block_fields(
    command: &str,
    prefix: &str,
) -> Result<HashMap<String, MclFieldType>, MclError> {
    let body = command
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(")"))
        .ok_or_else(|| {
            MclError::new(MclErrorKind::ParseFailed, "Block declaration is malformed")
        })?;
    let mut fields = HashMap::new();
    for declaration in body
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut parts = declaration.split_whitespace();
        let name = parts.next().ok_or_else(|| {
            MclError::new(MclErrorKind::ParseFailed, "Block field name is missing")
        })?;
        let kind = match parts.next() {
            Some("MESSAGE") => MclFieldType::Message,
            Some("TOOL_CALL") => MclFieldType::ToolCall,
            Some("TOOL") => MclFieldType::Tool,
            _ => {
                return Err(MclError::new(
                    MclErrorKind::TypeMismatch,
                    "MCL Block field type must be MESSAGE, TOOL_CALL, or TOOL",
                ))
            }
        };
        if parts.next().is_some() || fields.insert(name.to_owned(), kind).is_some() {
            return Err(MclError::new(
                MclErrorKind::ParseFailed,
                "MCL Block field declaration is invalid or duplicated",
            ));
        }
    }
    if fields.is_empty() {
        return Err(MclError::new(
            MclErrorKind::ParseFailed,
            "MCL Block must declare at least one field",
        ));
    }
    Ok(fields)
}

fn field_type(
    state: &MclDriverContext,
    block: &str,
    field: &str,
) -> Result<MclFieldType, MclError> {
    let fields = match block {
        "msg" => &state.message_fields,
        "tool" => &state.tool_fields,
        _ => {
            return Err(MclError::new(
                MclErrorKind::TypeMismatch,
                "Unknown MCL Block",
            ))
        }
    };
    fields.get(field).copied().ok_or_else(|| {
        MclError::new(
            MclErrorKind::TypeMismatch,
            format!("MCL field {block}.{field} is not declared"),
        )
    })
}

fn require_field_type(
    state: &MclDriverContext,
    block: &str,
    field: &str,
    expected: MclFieldType,
) -> Result<(), MclError> {
    let actual = field_type(state, block, field)?;
    if actual == expected {
        Ok(())
    } else {
        Err(MclError::new(
            MclErrorKind::TypeMismatch,
            format!("MCL field {block}.{field} has type {actual:?}, expected {expected:?}"),
        ))
    }
}

fn realtime_snapshot(state: &MclDriverContext) -> Option<Vec<AgentRealtimeMessage>> {
    (state.realtime_source.as_deref() == Some("req")).then(|| {
        state
            .history_conversation
            .iter()
            .chain(state.recent_conversation.iter())
            .map(|entry| AgentRealtimeMessage {
                message: entry.message.clone(),
                usage: entry.usage.clone(),
            })
            .collect()
    })
}

fn publish_realtime_snapshot(
    world: &mut World,
    agent: Entity,
    events: &app_runtime_plugin::RuntimeEventSender,
) {
    let snapshot = world
        .get_resource_mut::<MclDriverMailboxes>()
        .and_then(|mailboxes| mailboxes.states.get_mut(&agent))
        .and_then(|state| {
            let snapshot = realtime_snapshot(state)?;
            if snapshot == state.last_realtime_snapshot {
                return None;
            }
            state.last_realtime_snapshot = snapshot.clone();
            Some(snapshot)
        });
    if let Some(messages) = snapshot {
        events.send_event(AgentRealtimeContextWriteRequested { agent, messages });
    }
}

fn message_from_lua_json(value: serde_json::Value) -> Result<MclMessage, MclError> {
    let message_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| MclError::new(MclErrorKind::TypeMismatch, "MESSAGE binding has no type"))?;
    match message_type {
        "system" => Ok(MclMessage::new(
            Message::System {
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        "user" => Ok(MclMessage::new(
            Message::User {
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        "assistant" => Ok(MclMessage::new(
            Message::Assistant {
                reasoning: value
                    .get("reasoning")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                tool_calls: serde_json::from_value(
                    value
                        .get("tool_calls")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|_| MclError::new(MclErrorKind::TypeMismatch, "tool_calls is invalid"))?,
            },
            value
                .get("usage")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| MclError::new(MclErrorKind::TypeMismatch, "usage is invalid"))?,
        )),
        "tool" => Ok(MclMessage::new(
            Message::Tool {
                resource_id: serde_json::from_value(value.get("resource_id").cloned().ok_or_else(
                    || MclError::new(MclErrorKind::TypeMismatch, "resource_id is missing"),
                )?)
                .map_err(|_| MclError::new(MclErrorKind::TypeMismatch, "resource_id is invalid"))?,
                tool_call_id: value
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                content: value
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            None,
        )),
        _ => Err(MclError::new(
            MclErrorKind::TypeMismatch,
            "MESSAGE binding type is unsupported",
        )),
    }
}

fn message_to_lua_json(message: &MclMessage) -> serde_json::Value {
    let mut value = match &message.message {
        Message::System { content } => serde_json::json!({"type":"system", "content":content}),
        Message::User { content } => serde_json::json!({"type":"user", "content":content}),
        Message::Assistant {
            reasoning,
            content,
            tool_calls,
        } => serde_json::json!({
            "type":"assistant", "reasoning":reasoning, "content":content,
            "tool_calls":tool_calls
        }),
        Message::Tool {
            resource_id,
            tool_call_id,
            content,
        } => serde_json::json!({
            "type":"tool", "resource_id":resource_id, "tool_call_id":tool_call_id,
            "content":content
        }),
    };
    if let Some(usage) = &message.usage {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "usage".into(),
                serde_json::to_value(usage).expect("TokenUsage is serializable"),
            );
        }
    }
    value
}

pub(crate) fn mcl_command_system(world: &mut World) {
    let read_completions = world
        .event_reader::<AgentRealtimeContextReadCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for completion in read_completions {
        if let Some(reply) = world
            .get_resource_mut::<MclDriverMailboxes>()
            .expect("MclPlugin is installed")
            .realtime_reads
            .remove(&completion.id)
        {
            let result = completion
                .result
                .map(|entries| {
                    MclCommandValue::Json(serde_json::Value::Array(
                        entries
                            .iter()
                            .map(|entry| {
                                let mut value = message_to_lua_json(&MclMessage::new(
                                    entry.message.clone(),
                                    entry.usage.clone(),
                                ));
                                if let Some(usage) = &entry.usage {
                                    value["usage"] = serde_json::to_value(usage).unwrap();
                                }
                                value
                            })
                            .collect(),
                    ))
                })
                .map_err(|error| MclError::new(MclErrorKind::SourceReadFailed, error));
            let _ = reply.send(result);
        }
    }
    let commands = world
        .event_reader::<MclCommandReceived>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for command in commands {
        let mut reply = Some(command.reply);
        let command_text = command.command.trim().to_owned();
        let normalized = command_text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let result = if command_text == "EMIT EFFECT start" {
            let first_start = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .ready
                .insert(command.agent);
            if first_start {
                events.send_event(MclDriverReady {
                    agent: command.agent,
                });
            }
            let mailbox = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed");
            if let Some(message) = mailbox.queues.entry(command.agent).or_default().pop_front() {
                mailbox
                    .active_turns
                    .insert(command.agent, message.id.clone());
                Some(Ok(MclCommandValue::Json(message_to_lua_json(
                    &message.message,
                ))))
            } else {
                mailbox
                    .waiting
                    .insert(command.agent, reply.take().expect("reply exists"));
                None
            }
        } else if command_text == "EMIT EFFECT agent_info" {
            let context_window_tokens = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .map(|state| state.context_window_tokens)
                .ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::AgentMclMissing,
                        "Agent Driver context is missing",
                    )
                });
            Some(context_window_tokens.map(|context_window_tokens| {
                MclCommandValue::Json(serde_json::json!({
                    "model": {
                        "context_window_tokens": context_window_tokens,
                    }
                }))
            }))
        } else if let Some(import) = normalized.strip_prefix("IMPORT ") {
            let parsed = import
                .split_once(" AS ")
                .ok_or_else(|| MclError::new(MclErrorKind::ParseFailed, "IMPORT requires an alias"))
                .and_then(|(resource, alias)| {
                    ResourceId::parse(resource)
                        .map(|resource| (resource, alias.to_owned()))
                        .map_err(|_| {
                            MclError::new(
                                MclErrorKind::InvalidResourceId,
                                "IMPORT resource ID is invalid",
                            )
                        })
                });
            match parsed {
                Ok((resource, alias)) => {
                    world.send_event(MclResourceAliasDeclared {
                        agent: command.agent,
                        resource_id: resource.clone(),
                        alias: alias.clone(),
                    });
                    world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .states
                        .entry(command.agent)
                        .or_default()
                        .imports
                        .insert(alias, resource);
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if normalized.starts_with("CREATE MESSAGE_BLOCK msg (") {
            match parse_block_fields(&normalized, "CREATE MESSAGE_BLOCK msg (") {
                Ok(fields) => {
                    let state = world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .states
                        .entry(command.agent)
                        .or_default();
                    state.message_fields = fields;
                    state.message_block_created = true;
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if normalized.starts_with("CREATE TOOL_BLOCK tool (") {
            match parse_block_fields(&normalized, "CREATE TOOL_BLOCK tool (") {
                Ok(fields) => {
                    let state = world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .states
                        .entry(command.agent)
                        .or_default();
                    state.tool_fields = fields;
                    state.tool_block_created = true;
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if normalized.starts_with("CREATE REQUEST_BLOCK req (") {
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default()
                .request_block_created = true;
            Some(Ok(MclCommandValue::Unit))
        } else if normalized.starts_with("CREATE COMPACT_BLOCK com (") {
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default()
                .compact_block_created = true;
            Some(Ok(MclCommandValue::Unit))
        } else if let Some((target, block)) = command_text
            .strip_prefix("INJECT ? TO ")
            .and_then(|value| value.split_once(" FROM "))
        {
            let target = target.trim();
            let block = block.trim();
            let kind = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
                })
                .and_then(|state| field_type(state, block, target));
            match kind {
                Ok(MclFieldType::Message) => {
                    let message = command
                        .binding
                        .clone()
                        .ok_or_else(|| {
                            MclError::new(MclErrorKind::TypeMismatch, "MESSAGE binding is missing")
                        })
                        .and_then(message_from_lua_json);
                    match message {
                        Ok(message) => {
                            let state = world
                                .get_resource_mut::<MclDriverMailboxes>()
                                .expect("MclPlugin is installed")
                                .states
                                .entry(command.agent)
                                .or_default();
                            let values = match target {
                                "system" => Some(&mut state.system),
                                "history_conversation" => Some(&mut state.history_conversation),
                                "recent_conversation" | "conversation" => {
                                    Some(&mut state.recent_conversation)
                                }
                                "compact" => Some(&mut state.compact),
                                _ => None,
                            };
                            match values {
                                Some(values) => {
                                    values.push(message);
                                    Some(Ok(MclCommandValue::Unit))
                                }
                                None => Some(Err(MclError::new(
                                    MclErrorKind::TypeMismatch,
                                    "MESSAGE field has no runtime storage",
                                ))),
                            }
                        }
                        Err(error) => Some(Err(error)),
                    }
                }
                Ok(MclFieldType::ToolCall) => {
                    let call = command
                        .binding
                        .as_ref()
                        .and_then(|value| serde_json::from_value::<ToolCall>(value.clone()).ok());
                    match call {
                        Some(call) => {
                            world
                                .get_resource_mut::<MclDriverMailboxes>()
                                .expect("MclPlugin is installed")
                                .pending_tools
                                .entry(command.agent)
                                .or_default()
                                .push(call);
                            Some(Ok(MclCommandValue::Unit))
                        }
                        None => Some(Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            "TOOL_CALL binding is invalid",
                        ))),
                    }
                }
                Ok(MclFieldType::Tool) => {
                    let resource = command
                        .binding
                        .clone()
                        .and_then(|value| serde_json::from_value::<ResourceId>(value).ok());
                    match (target, resource) {
                        ("tool_default", Some(resource)) => {
                            world
                                .get_resource_mut::<MclDriverMailboxes>()
                                .expect("MclPlugin is installed")
                                .states
                                .entry(command.agent)
                                .or_default()
                                .tool_default
                                .push(resource);
                            Some(Ok(MclCommandValue::Unit))
                        }
                        ("tool_dynamic", Some(resource)) => {
                            world
                                .get_resource_mut::<MclDriverMailboxes>()
                                .expect("MclPlugin is installed")
                                .states
                                .entry(command.agent)
                                .or_default()
                                .tool_dynamic
                                .push(resource);
                            Some(Ok(MclCommandValue::Unit))
                        }
                        _ => Some(Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            "TOOL binding is invalid or has no runtime storage",
                        ))),
                    }
                }
                Err(error) => Some(Err(error)),
            }
        } else if let Some(target) = command_text
            .strip_prefix("INJECT ? COVER ")
            .and_then(|value| value.strip_suffix(" FROM msg"))
        {
            let declared = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
                })
                .and_then(|state| {
                    require_field_type(state, "msg", target.trim(), MclFieldType::Message)
                });
            let message = declared.and_then(|()| {
                command
                    .binding
                    .clone()
                    .ok_or_else(|| {
                        MclError::new(MclErrorKind::TypeMismatch, "COVER binding is missing")
                    })
                    .and_then(message_from_lua_json)
            });
            match message {
                Ok(message) => {
                    let state = world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .states
                        .entry(command.agent)
                        .or_default();
                    match target.trim() {
                        "history_conversation" => {
                            state.history_conversation = vec![message];
                            Some(Ok(MclCommandValue::Unit))
                        }
                        "recent_conversation" => {
                            state.recent_conversation = vec![message];
                            Some(Ok(MclCommandValue::Unit))
                        }
                        _ => Some(Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            "COVER target is unsupported",
                        ))),
                    }
                }
                Err(error) => Some(Err(error)),
            }
        } else if command_text
            == "INJECT SELECT tool_default FROM tool COVER tool_dynamic FROM tool"
        {
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            match require_field_type(state, "tool", "tool_default", MclFieldType::Tool).and_then(
                |()| require_field_type(state, "tool", "tool_dynamic", MclFieldType::Tool),
            ) {
                Ok(()) => {
                    state.tool_dynamic = state.tool_default.clone();
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if let Some(aliases) = command_text
            .strip_prefix("INJECT ")
            .and_then(|value| value.strip_suffix(" TO tool_default FROM tool"))
        {
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            match require_field_type(state, "tool", "tool_default", MclFieldType::Tool) {
                Err(error) => Some(Err(error)),
                Ok(()) => {
                    let mut resources = Vec::new();
                    let mut error = None;
                    for alias in aliases.split(',').map(str::trim) {
                        match state.imports.get(alias) {
                            Some(resource) => resources.push(resource.clone()),
                            None => {
                                error = Some(MclError::new(
                                    MclErrorKind::ImportMissing,
                                    format!("import alias {alias} is missing"),
                                ));
                                break;
                            }
                        }
                    }
                    if let Some(error) = error {
                        Some(Err(error))
                    } else {
                        state.tool_default.extend(resources);
                        Some(Ok(MclCommandValue::Unit))
                    }
                }
            }
        } else if let Some(spec) = command_text
            .strip_prefix("INJECT ")
            .and_then(|value| value.strip_suffix(" FROM msg"))
            .and_then(|value| value.split_once(" COVER "))
        {
            let (source, target) = spec;
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            let source_field = source
                .trim()
                .strip_prefix("SELECT ")
                .map(str::trim)
                .unwrap_or_default();
            let target_field = target.trim();
            let declaration = field_type(state, "msg", source_field).and_then(|source_type| {
                let target_type = field_type(state, "msg", target_field)?;
                if source_type == target_type {
                    Ok(source_type)
                } else {
                    Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        "COVER source and target fields must have the same type",
                    ))
                }
            });
            let values = match declaration {
                Err(error) => Err(error),
                Ok(MclFieldType::Message) => match source.trim() {
                    "SELECT recent_conversation" => Ok(state.recent_conversation.clone()),
                    "SELECT history_conversation" => Ok(state.history_conversation.clone()),
                    "SELECT compact" => Ok(state.compact.clone()),
                    _ => Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        "COVER source is unsupported",
                    )),
                },
                Ok(_) => Err(MclError::new(
                    MclErrorKind::TypeMismatch,
                    "COVER is not implemented for this field type",
                )),
            };
            match values {
                Err(error) => Some(Err(error)),
                Ok(values) => match target_field {
                    "history_conversation" => {
                        state.history_conversation = values;
                        Some(Ok(MclCommandValue::Unit))
                    }
                    "recent_conversation" => {
                        state.recent_conversation = values;
                        Some(Ok(MclCommandValue::Unit))
                    }
                    "compact" => {
                        state.compact = values;
                        Some(Ok(MclCommandValue::Unit))
                    }
                    _ => Some(Err(MclError::new(
                        MclErrorKind::TypeMismatch,
                        "COVER target is unsupported",
                    ))),
                },
            }
        } else if let Some(alias) = command_text
            .strip_prefix("INJECT ")
            .and_then(|value| value.strip_suffix(" TO system FROM msg"))
        {
            let system_prompt = world
                .get_component::<AgentMcl>(command.agent)
                .map(|mcl| mcl.system_prompt.clone())
                .unwrap_or_default();
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            if let Err(error) = require_field_type(state, "msg", "system", MclFieldType::Message) {
                Some(Err(error))
            } else if state.imports.contains_key(alias.trim()) {
                state.system.push(MclMessage::new(
                    Message::System {
                        content: system_prompt,
                    },
                    None,
                ));
                Some(Ok(MclCommandValue::Unit))
            } else {
                Some(Err(MclError::new(
                    MclErrorKind::ImportMissing,
                    "system prompt import is missing",
                )))
            }
        } else if let Some(alias) = command_text
            .strip_prefix("INJECT ")
            .and_then(|value| value.strip_suffix(" TO compact FROM msg"))
        {
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            if let Err(error) = require_field_type(state, "msg", "compact", MclFieldType::Message) {
                Some(Err(error))
            } else if !state.imports.contains_key(alias.trim()) {
                Some(Err(MclError::new(
                    MclErrorKind::ImportMissing,
                    "compact prompt import is missing",
                )))
            } else if let Some(content) = state.compact_prompt.clone() {
                state
                    .compact
                    .push(MclMessage::new(Message::User { content }, None));
                Some(Ok(MclCommandValue::Unit))
            } else {
                Some(Err(MclError::new(
                    MclErrorKind::SourceReadFailed,
                    "COMPACT.md could not be loaded",
                )))
            }
        } else if command_text == "DELETE pending_tool FROM msg WHERE id == ?" {
            let declared = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
                })
                .and_then(|state| {
                    require_field_type(state, "msg", "pending_tool", MclFieldType::ToolCall)
                });
            match declared {
                Err(error) => Some(Err(error)),
                Ok(()) => {
                    let id = command.binding.as_ref().and_then(|value| value.as_str());
                    match id {
                        Some(id) => {
                            let pending = world
                                .get_resource_mut::<MclDriverMailboxes>()
                                .expect("MclPlugin is installed")
                                .pending_tools
                                .entry(command.agent)
                                .or_default();
                            let before = pending.len();
                            pending.retain(|call| call.id != id);
                            if pending.len() + 1 == before {
                                Some(Ok(MclCommandValue::Unit))
                            } else {
                                Some(Err(MclError::new(
                                    MclErrorKind::InvalidMessageSequence,
                                    "pending ToolCall was not found exactly once",
                                )))
                            }
                        }
                        None => Some(Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            "pending_tool delete binding must be a string",
                        ))),
                    }
                }
            }
        } else if command_text == "SELECT pending_tool FROM msg" {
            let declared = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
                })
                .and_then(|state| {
                    require_field_type(state, "msg", "pending_tool", MclFieldType::ToolCall)
                });
            match declared {
                Err(error) => Some(Err(error)),
                Ok(()) => {
                    let values = world
                        .get_resource::<MclDriverMailboxes>()
                        .and_then(|mailboxes| mailboxes.pending_tools.get(&command.agent))
                        .cloned()
                        .unwrap_or_default();
                    Some(Ok(MclCommandValue::Json(
                        serde_json::to_value(values).expect("ToolCall is serializable"),
                    )))
                }
            }
        } else if let Some(field) = command_text
            .strip_prefix("SELECT ")
            .and_then(|value| value.strip_suffix(" FROM msg"))
        {
            let values = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.states.get(&command.agent))
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
                })
                .and_then(|state| {
                    require_field_type(state, "msg", field.trim(), MclFieldType::Message)?;
                    match field.trim() {
                        "recent_conversation" => Ok(state.recent_conversation.clone()),
                        "history_conversation" => Ok(state.history_conversation.clone()),
                        "compact" => Ok(state.compact.clone()),
                        _ => Err(MclError::new(
                            MclErrorKind::TypeMismatch,
                            "MESSAGE field has no runtime storage",
                        )),
                    }
                });
            Some(values.map(|values| {
                MclCommandValue::Json(
                    serde_json::to_value(
                        values.iter().map(message_to_lua_json).collect::<Vec<_>>(),
                    )
                    .expect("MclMessage is serializable"),
                )
            }))
        } else if let Some(field) = command_text
            .strip_prefix("DELETE ")
            .and_then(|value| value.strip_suffix(" FIRST FROM msg"))
        {
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            let declared = require_field_type(state, "msg", field.trim(), MclFieldType::Message);
            match declared {
                Err(error) => Some(Err(error)),
                Ok(()) => {
                    let removed = match field.trim() {
                        "recent_conversation" => state
                            .recent_conversation
                            .first()
                            .is_some()
                            .then(|| state.recent_conversation.remove(0)),
                        "history_conversation" => state
                            .history_conversation
                            .first()
                            .is_some()
                            .then(|| state.history_conversation.remove(0)),
                        _ => None,
                    };
                    if removed.is_some() {
                        Some(Ok(MclCommandValue::Unit))
                    } else {
                        Some(Err(MclError::new(
                            MclErrorKind::InvalidMessageSequence,
                            "message Block field is empty",
                        )))
                    }
                }
            }
        } else if command_text == "EMIT EFFECT realtime_load" {
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .realtime_reads
                .insert(command.id.clone(), reply.take().expect("reply exists"));
            events.send_event(AgentRealtimeContextReadRequested {
                id: command.id.clone(),
                agent: command.agent,
            });
            None
        } else if let Some(source) = command_text
            .strip_prefix("EMIT EFFECT realtime_source (")
            .and_then(|value| value.strip_suffix(')'))
        {
            let state = world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default();
            if source.trim() != "req" {
                Some(Err(MclError::new(
                    MclErrorKind::TypeMismatch,
                    "realtime source must be req",
                )))
            } else {
                state.realtime_source = Some("req".into());
                Some(Ok(MclCommandValue::Unit))
            }
        } else if command_text == "EMIT EFFECT history_append" {
            let message = command
                .binding
                .clone()
                .ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::TypeMismatch,
                        "history_append binding is missing",
                    )
                })
                .and_then(message_from_lua_json);
            match message {
                Ok(message) => {
                    let id = world
                        .get_resource::<MclDriverMailboxes>()
                        .and_then(|mailboxes| mailboxes.active_turns.get(&command.agent))
                        .cloned()
                        .unwrap_or_else(|| command.id.clone());
                    events.send_event(crate::MclHistoryAppendRequested {
                        id,
                        agent: command.agent,
                        message,
                    });
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if let Some(request) = command_text
            .strip_prefix("EMIT EFFECT inference (")
            .and_then(|value| value.strip_suffix(')'))
        {
            let turn_id = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.active_turns.get(&command.agent))
                .cloned()
                .unwrap_or_else(|| command.id.clone());
            events.send_event(MclEffectsProduced {
                id: turn_id,
                agent: command.agent,
                effects: vec![MclEffect::RequestInference {
                    request: request.trim().to_owned(),
                }],
            });
            Some(Ok(MclCommandValue::Unit))
        } else if let Some(request) = command_text
            .strip_prefix("EMIT EFFECT blocking_inference (")
            .and_then(|value| value.strip_suffix(')'))
        {
            events.send_event(crate::MclBlockingInferenceRequest {
                id: command.id,
                agent: command.agent,
                request: request.trim().to_owned(),
                reply: reply.take().expect("reply exists"),
            });
            None
        } else if command_text == "EMIT EFFECT finish" {
            let turn_id = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.active_turns.get(&command.agent))
                .cloned()
                .unwrap_or_else(|| command.id.clone());
            events.send_event(MclEffectsProduced {
                id: turn_id,
                agent: command.agent,
                effects: vec![MclEffect::FinishTurn],
            });
            Some(Ok(MclCommandValue::Unit))
        } else if command_text == "EMIT EFFECT tool_call ?" {
            let calls = command
                .binding
                .as_ref()
                .and_then(|value| serde_json::from_value::<Vec<ToolCall>>(value.clone()).ok())
                .ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::TypeMismatch,
                        "tool_call binding must be an array",
                    )
                });
            match calls {
                Ok(calls) => {
                    let turn_id = world
                        .get_resource::<MclDriverMailboxes>()
                        .and_then(|mailboxes| mailboxes.active_turns.get(&command.agent))
                        .cloned()
                        .unwrap_or_else(|| command.id.clone());
                    events.send_event(MclEffectsProduced {
                        id: turn_id,
                        agent: command.agent,
                        effects: vec![MclEffect::ExecuteTools { calls }],
                    });
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else {
            Some(Ok(MclCommandValue::Unit))
        };
        publish_realtime_snapshot(world, command.agent, &events);
        if let Some(result) = result {
            let _ = reply.expect("reply exists").send(result);
        }
    }

    let messages = world
        .event_reader::<MclRuntimeMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for message in messages {
        let mailbox = world
            .get_resource_mut::<MclDriverMailboxes>()
            .expect("MclPlugin is installed");
        if let Some(reply) = mailbox.waiting.remove(&message.agent) {
            mailbox
                .active_turns
                .insert(message.agent, message.id.clone());
            let _ = reply.send(Ok(MclCommandValue::Json(message_to_lua_json(
                &message.message,
            ))));
        } else {
            mailbox
                .queues
                .entry(message.agent)
                .or_default()
                .push_back(message);
        }
    }
    let _ = events;
}

pub(crate) fn start_driver_system(world: &mut World) {
    let starts = world
        .event_reader::<crate::StartMclDriver>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for start in starts {
        let source = world
            .get_resource_mut::<MclDriverMailboxes>()
            .expect("MclPlugin is installed")
            .drivers
            .remove(&start.agent);
        match source {
            Some(source) => {
                if let Err(error) =
                    crate::spawn_base_driver(start.agent, source, world.event_sender())
                {
                    world.emit_event(crate::MclDriverFailed {
                        agent: start.agent,
                        error,
                    });
                }
            }
            None => {
                tracing::warn!(agent = ?start.agent, "MCL Driver start was requested without a pending Driver")
            }
        }
    }
}

pub trait WorldMclExt {
    fn attach_agent_mcl(
        &mut self,
        agent: Entity,
        request: AttachAgentMclRequest,
    ) -> Result<Vec<MclEffect>, MclError>;

    fn execute_mcl_event(
        &mut self,
        agent: Entity,
        event: MclRuntimeEvent,
    ) -> Result<Vec<MclEffect>, MclError>;

    fn assemble_model_request(
        &self,
        agent: Entity,
        request: &str,
    ) -> Result<ModelRequestSnapshot, MclError>;

    fn grant_agent_resource(
        &mut self,
        agent: Entity,
        owner: MclCapabilityOwner,
        resource_id: ResourceId,
    ) -> Result<(), MclError>;

    fn revoke_agent_resource(
        &mut self,
        agent: Entity,
        owner: &MclCapabilityOwner,
        resource_id: &ResourceId,
    ) -> Result<(), MclError>;

    fn clear_agent_resource_owner(
        &mut self,
        agent: Entity,
        owner: &MclCapabilityOwner,
    ) -> Result<(), MclError>;
}

impl WorldMclExt for World {
    fn attach_agent_mcl(
        &mut self,
        agent: Entity,
        request: AttachAgentMclRequest,
    ) -> Result<Vec<MclEffect>, MclError> {
        if !self.is_alive(agent) {
            return Err(MclError::new(
                MclErrorKind::AgentMissing,
                "Agent is not alive",
            ));
        }
        if request.base.kind() != MclProgramKind::Base {
            return Err(MclError::new(
                MclErrorKind::InvalidProgramKind,
                "Agent Base MCL must use base context",
            ));
        }
        let driver_source = request.base.driver_source();
        let is_lua_driver = request.base.source().contains("handle(");
        let mut context = context_for_program(&request.base);
        let restored_block = request
            .base
            .blocks()
            .iter()
            .find(|block| {
                block.lifetime == MclBlockLifetime::Persistent
                    && matches!(
                        block.item_type,
                        crate::MclBlockType::Message | crate::MclBlockType::Context
                    )
            })
            .map(|block| block.name.clone());
        if !is_lua_driver && !request.restored_messages.is_empty() {
            let name = restored_block.ok_or_else(|| {
                MclError::new(
                    MclErrorKind::TypeMismatch,
                    "Base MCL has no persistent message Block for restored context",
                )
            })?;
            context
                .blocks
                .get_mut(&name)
                .expect("restored Block was selected from the same map")
                .items = restore_context_items(request.restored_messages.clone())?;
        }
        let plan_hash = request.base.plan_hash().clone();
        let default_visibility = request.default_visibility.clone();
        let mut mcl = AgentMcl {
            base: request.base,
            workflows: BTreeMap::new(),
            context,
            capabilities: MclCapabilityStore {
                default: request.default_visibility,
                grants: BTreeMap::new(),
            },
            system_prompt: request.system_prompt,
            plan_hash,
            plan_generation: 0,
        };
        let effects = if is_lua_driver {
            vec![MclEffect::ResolveResources {
                owner: MclCapabilityOwner::Base,
                resources: default_visibility.into_iter().collect(),
            }]
        } else {
            execute_on_state(&mut mcl, &MclRuntimeEvent::AgentCreated)?
        };
        if !self.insert_component(agent, mcl) {
            return Err(MclError::new(
                MclErrorKind::AgentMissing,
                "Agent MCL component could not be attached",
            ));
        }
        if is_lua_driver {
            let mailboxes = self
                .get_resource_mut::<MclDriverMailboxes>()
                .ok_or_else(|| {
                    MclError::new(MclErrorKind::AgentMclMissing, "Driver state is missing")
                })?;
            let state = mailboxes.states.entry(agent).or_default();
            state.context_window_tokens = request.context_window_tokens;
            state.compact_prompt = driver_source
                .origin()
                .parent()
                .map(|root| root.join("COMPACT.md"))
                .filter(|path| path.is_file())
                .and_then(|path| std::fs::read_to_string(path).ok());
            mailboxes.drivers.insert(agent, driver_source);
        }
        // Lua Base Driver is the authoritative control loop. It starts after
        // AgentMcl exists so command transactions can address this Agent.
        Ok(effects)
    }

    fn execute_mcl_event(
        &mut self,
        agent: Entity,
        event: MclRuntimeEvent,
    ) -> Result<Vec<MclEffect>, MclError> {
        let current = self
            .get_component::<AgentMcl>(agent)
            .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?;
        let mut transaction = current.clone();
        let effects = execute_on_state(&mut transaction, &event)?;
        *self
            .get_component_mut::<AgentMcl>(agent)
            .expect("AgentMcl existence was checked") = transaction;
        Ok(effects)
    }

    fn assemble_model_request(
        &self,
        agent: Entity,
        request_name: &str,
    ) -> Result<ModelRequestSnapshot, MclError> {
        let mcl = self
            .get_component::<AgentMcl>(agent)
            .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?;
        if mcl.base.source().contains("handle(") {
            let mailboxes = self.get_resource::<MclDriverMailboxes>().ok_or_else(|| {
                MclError::new(MclErrorKind::AgentMclMissing, "Driver state is missing")
            })?;
            let state = mailboxes.states.get(&agent).ok_or_else(|| {
                MclError::new(MclErrorKind::AgentMclMissing, "Driver context is missing")
            })?;
            if !state.message_block_created
                || !state.tool_block_created
                || (request_name == "req" && !state.request_block_created)
                || (request_name == "com" && !state.compact_block_created)
                || !matches!(request_name, "req" | "com")
            {
                return Err(MclError::new(
                    MclErrorKind::TypeMismatch,
                    "MCL request block is not initialized",
                ));
            }
            if mailboxes
                .pending_tools
                .get(&agent)
                .is_some_and(|pending| !pending.is_empty())
            {
                return Err(MclError::new(
                    MclErrorKind::InvalidMessageSequence,
                    "inference cannot start while pending_tool is not empty",
                ));
            }
            let system = state
                .system
                .iter()
                .filter_map(|message| match &message.message {
                    Message::System { content } => Some(content.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let message_source = if request_name == "com" {
                state
                    .history_conversation
                    .iter()
                    .chain(state.compact.iter())
                    .collect::<Vec<_>>()
            } else {
                state
                    .history_conversation
                    .iter()
                    .chain(state.recent_conversation.iter())
                    .collect::<Vec<_>>()
            };
            let messages = message_source
                .into_iter()
                .map(|message| message.message.clone())
                .collect();
            let visible_resources = state
                .tool_dynamic
                .iter()
                .filter(|resource| mcl.capabilities.is_visible(resource))
                .cloned()
                .collect();
            return Ok(ModelRequestSnapshot {
                plan_hash: mcl.plan_hash.clone(),
                plan_generation: mcl.plan_generation,
                base_program_hash: mcl.base.plan_hash().clone(),
                workflow_program_hashes: BTreeMap::new(),
                system,
                messages,
                visible_resources,
                provenance: MclSnapshotProvenance {
                    request: request_name.to_owned(),
                    views: vec![
                        "msg.system".into(),
                        "msg.conversation".into(),
                        "tool.tool_dynamic".into(),
                    ],
                },
            });
        }
        let (program, context, request_name) =
            if let Some(qualified) = request_name.strip_prefix("workflow::") {
                let (instance_id, local_name) = qualified.rsplit_once("::").ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::TypeMismatch,
                        "Workflow MCL Request reference is invalid",
                    )
                })?;
                let workflow = mcl
                    .workflows
                    .iter()
                    .find(|(id, _)| id.as_str() == instance_id)
                    .map(|(_, workflow)| workflow)
                    .ok_or_else(|| {
                        MclError::new(
                            MclErrorKind::WorkflowMissing,
                            "Workflow MCL Request owner is not attached",
                        )
                    })?;
                (workflow.program.as_ref(), &workflow.blocks, local_name)
            } else {
                (mcl.base.as_ref(), &mcl.context, request_name)
            };
        let request = program
            .requests()
            .iter()
            .find(|request| request.name == request_name)
            .ok_or_else(|| {
                MclError::new(
                    MclErrorKind::TypeMismatch,
                    format!("MCL Request {request_name} does not exist"),
                )
            })?;
        let message_view = program
            .views()
            .iter()
            .find(|view| view.name == request.messages)
            .ok_or_else(|| MclError::new(MclErrorKind::TypeMismatch, "Message View is missing"))?;
        let blocks = match &message_view.kind {
            MclViewKind::Messages { blocks } => blocks,
            _ => {
                return Err(MclError::new(
                    MclErrorKind::TypeMismatch,
                    "request.messages does not reference a Message View",
                ));
            }
        };
        let mut messages = Vec::new();
        for block in blocks {
            let block = context.blocks.get(block).ok_or_else(|| {
                MclError::new(MclErrorKind::TypeMismatch, "Message View Block is missing")
            })?;
            append_block_messages(block, &mut messages)?;
        }
        validate_message_view(&messages)?;
        let workflow_program_hashes = mcl
            .workflows
            .iter()
            .map(|(id, workflow)| (id.clone(), workflow.program.plan_hash().clone()))
            .collect();
        Ok(ModelRequestSnapshot {
            plan_hash: mcl.plan_hash.clone(),
            plan_generation: mcl.plan_generation,
            base_program_hash: mcl.base.plan_hash().clone(),
            workflow_program_hashes,
            system: mcl.system_prompt.clone(),
            messages,
            visible_resources: mcl.capabilities.visible_resources().cloned().collect(),
            provenance: MclSnapshotProvenance {
                request: request_name.to_owned(),
                views: vec![request.messages.clone(), request.tools.clone()],
            },
        })
    }

    fn grant_agent_resource(
        &mut self,
        agent: Entity,
        owner: MclCapabilityOwner,
        resource_id: ResourceId,
    ) -> Result<(), MclError> {
        self.get_component_mut::<AgentMcl>(agent)
            .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?
            .capabilities
            .grant(owner, resource_id);
        Ok(())
    }

    fn revoke_agent_resource(
        &mut self,
        agent: Entity,
        owner: &MclCapabilityOwner,
        resource_id: &ResourceId,
    ) -> Result<(), MclError> {
        self.get_component_mut::<AgentMcl>(agent)
            .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?
            .capabilities
            .revoke(owner, resource_id);
        Ok(())
    }

    fn clear_agent_resource_owner(
        &mut self,
        agent: Entity,
        owner: &MclCapabilityOwner,
    ) -> Result<(), MclError> {
        self.get_component_mut::<AgentMcl>(agent)
            .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?
            .capabilities
            .clear_owner(owner);
        Ok(())
    }
}

fn context_for_program(program: &MclProgram) -> MclContextStore {
    MclContextStore {
        blocks: program
            .blocks()
            .iter()
            .map(|definition| {
                (
                    definition.name.clone(),
                    MclBlock {
                        definition: definition.clone(),
                        items: Vec::new(),
                    },
                )
            })
            .collect(),
    }
}

fn execute_on_state(
    mcl: &mut AgentMcl,
    event: &MclRuntimeEvent,
) -> Result<Vec<MclEffect>, MclError> {
    let event_name = event_name(event);
    let mut handlers = mcl
        .base
        .handlers()
        .iter()
        .enumerate()
        .filter(|(_, handler)| handler.event == event_name)
        .map(|(index, handler)| {
            (
                handler.priority,
                String::new(),
                index,
                MclCapabilityOwner::Base,
                handler.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (instance_id, workflow) in &mcl.workflows {
        handlers.extend(
            workflow
                .program
                .handlers()
                .iter()
                .enumerate()
                .filter(|(_, handler)| handler.event == event_name)
                .map(|(index, handler)| {
                    (
                        handler.priority,
                        instance_id.as_str().to_owned(),
                        index,
                        MclCapabilityOwner::Workflow(instance_id.clone()),
                        handler.clone(),
                    )
                }),
        );
    }
    handlers.sort_by(|left, right| (&left.0, &left.1, left.2).cmp(&(&right.0, &right.1, right.2)));

    let mut effects = Vec::new();
    let mut finish_turn = false;
    for (_, _, _, owner, handler) in handlers {
        if !predicate_matches(&handler.predicate, event) {
            continue;
        }
        for statement in handler.statements {
            execute_statement(
                mcl,
                event,
                &owner,
                statement,
                &mut effects,
                &mut finish_turn,
            )?;
        }
    }
    if finish_turn {
        for block in mcl.context.blocks.values_mut() {
            if block.definition.lifetime == MclBlockLifetime::Turn {
                block.items.clear();
            }
        }
    }
    Ok(effects)
}

fn execute_statement(
    mcl: &mut AgentMcl,
    event: &MclRuntimeEvent,
    owner: &MclCapabilityOwner,
    statement: MclStatement,
    effects: &mut Vec<MclEffect>,
    finish_turn: &mut bool,
) -> Result<(), MclError> {
    match statement {
        MclStatement::AppendEntry { block } => {
            target_block_mut(mcl, owner, &block)?
                .items
                .push(MclContextItem::Message(event_message(event)?.clone()));
        }
        MclStatement::AppendExchange { block } => {
            let message = event_message(event)?.clone();
            let Message::Assistant { tool_calls, .. } = &message else {
                return Err(MclError::new(
                    MclErrorKind::InvalidEvent,
                    "only Assistant messages can begin a ToolExchange",
                ));
            };
            if tool_calls.is_empty() {
                return Err(MclError::new(
                    MclErrorKind::InvalidEvent,
                    "a ToolExchange requires at least one ToolCall",
                ));
            }
            let unique = tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<BTreeSet<_>>();
            if unique.len() != tool_calls.len() {
                return Err(MclError::new(
                    MclErrorKind::InvalidMessageSequence,
                    "Assistant ToolCall IDs are not unique",
                ));
            }
            target_block_mut(mcl, owner, &block)?
                .items
                .push(MclContextItem::ToolExchange(MclToolExchange {
                    assistant: message,
                    responses: BTreeMap::new(),
                    state: MclToolExchangeState::Open,
                }));
        }
        MclStatement::AppendEntryToExchange => {
            let message = event_message(event)?.clone();
            let Message::Tool { tool_call_id, .. } = &message else {
                return Err(MclError::new(
                    MclErrorKind::InvalidEvent,
                    "only Tool messages can be appended to an exchange",
                ));
            };
            let tool_call_id = tool_call_id.clone();
            append_tool_response(mcl, owner, &tool_call_id, message)?;
        }
        MclStatement::ClearBlock { block } => {
            target_block_mut(mcl, owner, &block)?.items.clear();
        }
        MclStatement::RestoreDefaultCapabilities => {
            let defaults = mcl.capabilities.default.iter().cloned().collect::<Vec<_>>();
            effects.push(MclEffect::ResolveResources {
                owner: owner.clone(),
                resources: defaults,
            });
        }
        MclStatement::ShowResource { resource } => {
            let resource = resolve_resource(event, resource)?;
            effects.push(MclEffect::ResolveResources {
                owner: owner.clone(),
                resources: vec![resource],
            });
        }
        MclStatement::HideResource { resource } => {
            let resource = resolve_resource(event, resource)?;
            mcl.capabilities.revoke(owner, &resource);
        }
        MclStatement::ClearCapabilities => mcl.capabilities.clear_owner(owner),
        MclStatement::EmitInference { request } => {
            let request = match owner {
                MclCapabilityOwner::Workflow(instance_id) => {
                    format!("workflow::{instance_id}::{request}")
                }
                _ => request,
            };
            effects.push(MclEffect::RequestInference { request });
        }
        MclStatement::EmitTools => {
            effects.push(MclEffect::ExecuteTools {
                calls: event_tool_calls(event)?.to_vec(),
            });
        }
        MclStatement::FinishTurn => {
            effects.push(MclEffect::FinishTurn);
            *finish_turn = true;
        }
    }
    Ok(())
}

fn append_tool_response(
    mcl: &mut AgentMcl,
    owner: &MclCapabilityOwner,
    tool_call_id: &str,
    message: Message,
) -> Result<(), MclError> {
    let blocks = match owner {
        MclCapabilityOwner::Base | MclCapabilityOwner::External(_) => &mut mcl.context.blocks,
        MclCapabilityOwner::Workflow(instance_id) => {
            &mut mcl
                .workflows
                .get_mut(instance_id)
                .ok_or_else(|| MclError::new(MclErrorKind::WorkflowMissing, "Workflow is missing"))?
                .blocks
                .blocks
        }
    };
    for block in blocks.values_mut() {
        for item in block.items.iter_mut().rev() {
            let MclContextItem::ToolExchange(exchange) = item else {
                continue;
            };
            if exchange.state != MclToolExchangeState::Open {
                continue;
            }
            let Message::Assistant { tool_calls, .. } = &exchange.assistant else {
                unreachable!("MclToolExchange always owns an Assistant message");
            };
            if !tool_calls.iter().any(|call| call.id == tool_call_id) {
                continue;
            }
            if exchange
                .responses
                .insert(tool_call_id.to_owned(), message)
                .is_some()
            {
                return Err(MclError::new(
                    MclErrorKind::InvalidMessageSequence,
                    "Tool response ID is duplicated",
                ));
            }
            if exchange.responses.len() == tool_calls.len() {
                exchange.state = MclToolExchangeState::Closed;
            }
            return Ok(());
        }
    }
    Err(MclError::new(
        MclErrorKind::InvalidMessageSequence,
        "Tool response does not match an open ToolExchange",
    ))
}

fn restore_context_items(messages: Vec<Message>) -> Result<Vec<MclContextItem>, MclError> {
    let mut items = Vec::new();
    for message in messages {
        match message {
            Message::Assistant { ref tool_calls, .. } if !tool_calls.is_empty() => {
                if items.iter().any(|item| {
                    matches!(item, MclContextItem::ToolExchange(exchange) if exchange.state == MclToolExchangeState::Open)
                }) {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "restored context contains overlapping ToolExchanges",
                    ));
                }
                items.push(MclContextItem::ToolExchange(MclToolExchange {
                    assistant: message,
                    responses: BTreeMap::new(),
                    state: MclToolExchangeState::Open,
                }));
            }
            Message::Tool {
                ref tool_call_id, ..
            } => {
                let exchange = items.iter_mut().rev().find_map(|item| match item {
                    MclContextItem::ToolExchange(exchange)
                        if exchange.state == MclToolExchangeState::Open =>
                    {
                        Some(exchange)
                    }
                    _ => None,
                });
                let Some(exchange) = exchange else {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "restored Tool response has no open ToolExchange",
                    ));
                };
                let Message::Assistant { tool_calls, .. } = &exchange.assistant else {
                    unreachable!("MclToolExchange always owns an Assistant message");
                };
                if !tool_calls.iter().any(|call| call.id == *tool_call_id)
                    || exchange
                        .responses
                        .insert(tool_call_id.clone(), message)
                        .is_some()
                {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "restored Tool response is unknown or duplicated",
                    ));
                }
                if exchange.responses.len() == tool_calls.len() {
                    exchange.state = MclToolExchangeState::Closed;
                }
            }
            other => {
                if items.iter_mut().rev().any(|item| match item {
                    MclContextItem::ToolExchange(exchange)
                        if exchange.state == MclToolExchangeState::Open =>
                    {
                        exchange.state = MclToolExchangeState::Interrupted;
                        true
                    }
                    _ => false,
                }) {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "restored ToolExchange is interrupted by a non-Tool message",
                    ));
                }
                items.push(MclContextItem::Message(other));
            }
        }
    }
    Ok(items)
}

fn append_block_messages(block: &MclBlock, output: &mut Vec<Message>) -> Result<(), MclError> {
    for item in &block.items {
        match item {
            MclContextItem::Message(message) => output.push(message.clone()),
            MclContextItem::ToolExchange(exchange) => {
                if exchange.state != MclToolExchangeState::Closed {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "Model Request cannot contain an open or interrupted ToolExchange",
                    ));
                }
                output.push(exchange.assistant.clone());
                output.extend(exchange.responses().cloned());
            }
        }
    }
    Ok(())
}

fn target_block_mut<'a>(
    mcl: &'a mut AgentMcl,
    owner: &MclCapabilityOwner,
    block: &str,
) -> Result<&'a mut MclBlock, MclError> {
    match owner {
        MclCapabilityOwner::Base | MclCapabilityOwner::External(_) => mcl
            .context
            .blocks
            .get_mut(block)
            .ok_or_else(|| MclError::new(MclErrorKind::TypeMismatch, "MCL Block is missing")),
        MclCapabilityOwner::Workflow(instance_id) => mcl
            .workflows
            .get_mut(instance_id)
            .and_then(|workflow| workflow.blocks.blocks.get_mut(block))
            .ok_or_else(|| {
                MclError::new(MclErrorKind::TypeMismatch, "Workflow MCL Block is missing")
            }),
    }
}

fn resolve_resource(
    event: &MclRuntimeEvent,
    expression: MclResourceExpression,
) -> Result<ResourceId, MclError> {
    match expression {
        MclResourceExpression::Literal(resource) => Ok(resource),
        MclResourceExpression::EventResource => match event {
            MclRuntimeEvent::ResourceInjected { resource_id }
            | MclRuntimeEvent::ResourceRemoved { resource_id } => Ok(resource_id.clone()),
            _ => Err(MclError::new(
                MclErrorKind::InvalidEvent,
                "event.resource is unavailable for this event",
            )),
        },
    }
}

fn predicate_matches(predicate: &MclPredicate, event: &MclRuntimeEvent) -> bool {
    match predicate {
        MclPredicate::Always => true,
        MclPredicate::ToolCallsEmpty => event_tool_calls(event).is_ok_and(|calls| calls.is_empty()),
        MclPredicate::ToolCallsNotEmpty => {
            event_tool_calls(event).is_ok_and(|calls| !calls.is_empty())
        }
    }
}

fn event_message(event: &MclRuntimeEvent) -> Result<&Message, MclError> {
    match event {
        MclRuntimeEvent::UserMessage { entry }
        | MclRuntimeEvent::AssistantMessage { entry }
        | MclRuntimeEvent::ToolMessage { entry } => Ok(entry),
        _ => Err(MclError::new(
            MclErrorKind::InvalidEvent,
            "event.entry is unavailable for this event",
        )),
    }
}

fn event_tool_calls(event: &MclRuntimeEvent) -> Result<&[ToolCall], MclError> {
    match event_message(event)? {
        Message::Assistant { tool_calls, .. } => Ok(tool_calls),
        _ => Err(MclError::new(
            MclErrorKind::InvalidEvent,
            "event.tool_calls is unavailable for this event",
        )),
    }
}

fn event_name(event: &MclRuntimeEvent) -> &'static str {
    match event {
        MclRuntimeEvent::AgentCreated => "agent.created",
        MclRuntimeEvent::UserMessage { .. } => "message.user",
        MclRuntimeEvent::AssistantMessage { .. } => "message.assistant",
        MclRuntimeEvent::ToolMessage { .. } => "message.tool",
        MclRuntimeEvent::ToolBatchCompleted => "tool.batch.completed",
        MclRuntimeEvent::ToolBatchFailed => "tool.batch.failed",
        MclRuntimeEvent::InferenceFailed => "inference.failed",
        MclRuntimeEvent::TurnAborted => "turn.aborted",
        MclRuntimeEvent::ResourceInjected { .. } => "resource.injected",
        MclRuntimeEvent::ResourceRemoved { .. } => "resource.removed",
        MclRuntimeEvent::WorkflowAttached { .. } => "workflow.attached",
        MclRuntimeEvent::WorkflowDetaching { .. } => "workflow.detaching",
    }
}

fn validate_message_view(messages: &[Message]) -> Result<(), MclError> {
    let mut open: Option<BTreeSet<&str>> = None;
    for message in messages {
        match message {
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                if open.is_some() {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "Assistant tool calls begin before the previous ToolExchange is closed",
                    ));
                }
                let ids = tool_calls
                    .iter()
                    .map(|call| call.id.as_str())
                    .collect::<BTreeSet<_>>();
                if ids.len() != tool_calls.len() {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "Assistant ToolCall IDs are not unique",
                    ));
                }
                open = Some(ids);
            }
            Message::Tool { tool_call_id, .. } => {
                let Some(ids) = open.as_mut() else {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "Tool response has no preceding Assistant ToolCall",
                    ));
                };
                if !ids.remove(tool_call_id.as_str()) {
                    return Err(MclError::new(
                        MclErrorKind::InvalidMessageSequence,
                        "Tool response ID is unknown or duplicated",
                    ));
                }
                if ids.is_empty() {
                    open = None;
                }
            }
            _ if open.is_some() => {
                return Err(MclError::new(
                    MclErrorKind::InvalidMessageSequence,
                    "ToolExchange is interrupted by a non-Tool message",
                ));
            }
            _ => {}
        }
    }
    if open.is_some() {
        return Err(MclError::new(
            MclErrorKind::InvalidMessageSequence,
            "Model Request cannot contain an open ToolExchange",
        ));
    }
    Ok(())
}

fn recompute_plan_hash(mcl: &mut AgentMcl) {
    let mut parts = vec![mcl.base.plan_hash().as_str().as_bytes()];
    for workflow in mcl.workflows.values() {
        parts.push(workflow.program.plan_hash().as_str().as_bytes());
    }
    mcl.plan_hash = MclHash::digest(parts);
    mcl.plan_generation = mcl.plan_generation.saturating_add(1);
}

pub(crate) fn workflow_control_system(world: &mut World) {
    if !world.contains_resource::<MclPluginInstalled>() {
        return;
    }
    let attaches = world
        .event_reader::<AttachWorkflowMcl>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let detaches = world
        .event_reader::<DetachWorkflowMcl>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let home_root = world
        .get_resource::<MclRuntime>()
        .expect("MclPlugin is installed")
        .home_root
        .as_ref()
        .clone();

    for event in attaches {
        let result = attach_workflow(world, &event, &home_root);
        match result {
            Ok((instance_id, effects)) => {
                world.emit_event(WorkflowMclAttached {
                    id: event.id.clone(),
                    agent: event.agent,
                    instance_id: instance_id.clone(),
                    resource_id: event.resource_id,
                });
                world.emit_event(MclEffectsProduced {
                    id: event.id,
                    agent: event.agent,
                    effects,
                });
            }
            Err(error) => world.emit_event(WorkflowMclAttachFailed {
                id: event.id,
                agent: event.agent,
                resource_id: event.resource_id,
                error,
            }),
        }
    }
    for event in detaches {
        let result = detach_workflow(world, &event);
        match result {
            Ok(removed_resources) => world.emit_event(WorkflowMclDetached {
                id: event.id,
                agent: event.agent,
                instance_id: event.instance_id,
                removed_resources,
            }),
            Err(error) => world.emit_event(WorkflowMclDetachFailed {
                id: event.id,
                agent: event.agent,
                instance_id: event.instance_id,
                error,
            }),
        }
    }
}

fn attach_workflow(
    world: &mut World,
    event: &AttachWorkflowMcl,
    home_root: &PathBuf,
) -> Result<(WorkflowInstanceId, Vec<MclEffect>), MclError> {
    let program = load_mcl_program(
        &[
            event.project_root.clone(),
            event.image_root.clone(),
            home_root.clone(),
        ],
        &event.resource_id,
        MclProgramKind::Workflow,
    )?;
    let instance_id = WorkflowInstanceId::new(event.id.clone())?;
    let current = world
        .get_component::<AgentMcl>(event.agent)
        .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?;
    if current.workflows.contains_key(&instance_id) {
        return Err(MclError::new(
            MclErrorKind::DuplicateName,
            "Workflow instance ID is already attached",
        ));
    }
    let before = current.clone();
    let mcl = world
        .get_component_mut::<AgentMcl>(event.agent)
        .expect("AgentMcl existence was checked");
    mcl.workflows.insert(
        instance_id.clone(),
        WorkflowMclInstance {
            id: instance_id.clone(),
            resource_id: event.resource_id.clone(),
            blocks: context_for_program(&program),
            program,
            pending_effects: BTreeSet::new(),
        },
    );
    recompute_plan_hash(mcl);
    let effects = match world.execute_mcl_event(
        event.agent,
        MclRuntimeEvent::WorkflowAttached {
            instance_id: instance_id.clone(),
        },
    ) {
        Ok(effects) => effects,
        Err(error) => {
            *world
                .get_component_mut::<AgentMcl>(event.agent)
                .expect("AgentMcl existed before Workflow attach") = before;
            return Err(error);
        }
    };
    Ok((instance_id, effects))
}

fn detach_workflow(
    world: &mut World,
    event: &DetachWorkflowMcl,
) -> Result<Vec<ResourceId>, MclError> {
    let before = world
        .get_component::<AgentMcl>(event.agent)
        .ok_or_else(|| MclError::new(MclErrorKind::AgentMclMissing, "AgentMcl is missing"))?
        .clone();
    let workflow = before.workflows.get(&event.instance_id).ok_or_else(|| {
        MclError::new(
            MclErrorKind::WorkflowMissing,
            "Workflow instance is not attached",
        )
    })?;
    if !workflow.pending_effects.is_empty() {
        return Err(MclError::new(
            MclErrorKind::WorkflowBusy,
            "Workflow has pending effects",
        ));
    }
    if let Err(error) = world.execute_mcl_event(
        event.agent,
        MclRuntimeEvent::WorkflowDetaching {
            instance_id: event.instance_id.clone(),
        },
    ) {
        *world
            .get_component_mut::<AgentMcl>(event.agent)
            .expect("AgentMcl existed before Workflow detach") = before;
        return Err(error);
    }
    let mcl = world
        .get_component_mut::<AgentMcl>(event.agent)
        .expect("AgentMcl existence was checked");
    let before_visible = mcl
        .capabilities
        .visible_resources()
        .cloned()
        .collect::<BTreeSet<_>>();
    mcl.workflows.remove(&event.instance_id);
    mcl.capabilities
        .clear_owner(&MclCapabilityOwner::Workflow(event.instance_id.clone()));
    let after_visible = mcl
        .capabilities
        .visible_resources()
        .cloned()
        .collect::<BTreeSet<_>>();
    let removed = before_visible
        .difference(&after_visible)
        .cloned()
        .collect::<Vec<_>>();
    recompute_plan_hash(mcl);
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use margatroid_types::TokenUsage;

    use super::{message_from_lua_json, message_to_lua_json, parse_block_fields, MclFieldType};
    use crate::MclMessage;

    #[test]
    fn assistant_usage_survives_lua_message_round_trip() {
        let message = MclMessage::new(
            margatroid_types::Message::Assistant {
                reasoning: Some("reasoning".into()),
                content: Some("answer".into()),
                tool_calls: Vec::new(),
            },
            Some(TokenUsage {
                input_tokens: 800_000,
                output_tokens: 100,
                cache_hit_tokens: 700_000,
            }),
        );

        let restored = message_from_lua_json(message_to_lua_json(&message)).unwrap();

        assert_eq!(restored, message);
    }

    #[test]
    fn block_fields_accept_only_the_three_mcl_value_types() {
        let fields = parse_block_fields(
            "CREATE MESSAGE_BLOCK msg ( system MESSAGE, pending_tool TOOL_CALL, visible TOOL )",
            "CREATE MESSAGE_BLOCK msg (",
        )
        .unwrap();
        assert_eq!(fields.get("system"), Some(&MclFieldType::Message));
        assert_eq!(fields.get("pending_tool"), Some(&MclFieldType::ToolCall));
        assert_eq!(fields.get("visible"), Some(&MclFieldType::Tool));
        assert!(parse_block_fields(
            "CREATE MESSAGE_BLOCK msg ( invalid STRING )",
            "CREATE MESSAGE_BLOCK msg (",
        )
        .is_err());
    }
}
