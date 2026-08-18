use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use app_runtime_plugin::WorldEventExt;
use core_plugin::{Entity, World};
use margatroid_types::{Message, ResourceId, ToolCall};

use crate::syntax::{
    MclBlockLifetime, MclPredicate, MclResourceExpression, MclStatement, MclViewKind,
};
use crate::{
    load_mcl_program, AgentMcl, AttachAgentMclRequest, AttachWorkflowMcl, DetachWorkflowMcl,
    MclBlock, MclCapabilityOwner, MclCapabilityStore, MclCommandReceived, MclCommandValue,
    MclContextItem, MclContextStore, MclDriverReady, MclEffect, MclEffectsProduced, MclError,
    MclErrorKind, MclHash, MclPluginInstalled, MclProgram, MclProgramKind, MclRuntime,
    MclRuntimeEvent, MclRuntimeMessage, MclSnapshotProvenance, MclToolExchange,
    MclToolExchangeState, ModelRequestSnapshot, WorkflowInstanceId, WorkflowMclAttachFailed,
    WorkflowMclAttached, WorkflowMclDetachFailed, WorkflowMclDetached, WorkflowMclInstance,
};

#[derive(Default)]
pub(crate) struct MclDriverMailboxes {
    queues: HashMap<Entity, VecDeque<MclRuntimeMessage>>,
    waiting: HashMap<Entity, Sender<Result<MclCommandValue, MclError>>>,
    active_turns: HashMap<Entity, String>,
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
    system: Vec<Message>,
    conversation: Vec<Message>,
    tool_default: Vec<ResourceId>,
    tool_dynamic: Vec<ResourceId>,
    imports: HashMap<String, ResourceId>,
}

fn message_from_lua_json(value: serde_json::Value) -> Result<Message, MclError> {
    let message_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| MclError::new(MclErrorKind::TypeMismatch, "MESSAGE binding has no type"))?;
    match message_type {
        "system" => Ok(Message::System {
            content: value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "user" => Ok(Message::User {
            content: value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "assistant" => Ok(Message::Assistant {
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
        }),
        "tool" => Ok(Message::Tool {
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
        }),
        _ => Err(MclError::new(
            MclErrorKind::TypeMismatch,
            "MESSAGE binding type is unsupported",
        )),
    }
}

fn message_to_lua_json(message: &Message) -> serde_json::Value {
    match message {
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
    }
}

pub(crate) fn mcl_command_system(world: &mut World) {
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
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default()
                .message_block_created = true;
            Some(Ok(MclCommandValue::Unit))
        } else if normalized.starts_with("CREATE TOOL_BLOCK tool (") {
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default()
                .tool_block_created = true;
            Some(Ok(MclCommandValue::Unit))
        } else if normalized.starts_with("CREATE REQUEST_BLOCK req (") {
            world
                .get_resource_mut::<MclDriverMailboxes>()
                .expect("MclPlugin is installed")
                .states
                .entry(command.agent)
                .or_default()
                .request_block_created = true;
            Some(Ok(MclCommandValue::Unit))
        } else if command_text == "INJECT ? TO conversation FROM msg" {
            match command
                .binding
                .clone()
                .ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::TypeMismatch,
                        "conversation binding is missing",
                    )
                })
                .and_then(message_from_lua_json)
            {
                Ok(message) => {
                    world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .states
                        .entry(command.agent)
                        .or_default()
                        .conversation
                        .push(message);
                    Some(Ok(MclCommandValue::Unit))
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
            state.tool_dynamic = state.tool_default.clone();
            Some(Ok(MclCommandValue::Unit))
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
            if state.imports.contains_key(alias.trim()) {
                state.system.push(Message::System {
                    content: system_prompt,
                });
                Some(Ok(MclCommandValue::Unit))
            } else {
                Some(Err(MclError::new(
                    MclErrorKind::ImportMissing,
                    "system prompt import is missing",
                )))
            }
        } else if command_text == "INJECT ? TO pending_tool FROM msg" {
            let call = command
                .binding
                .as_ref()
                .and_then(|value| serde_json::from_value::<ToolCall>(value.clone()).ok())
                .ok_or_else(|| {
                    MclError::new(
                        MclErrorKind::TypeMismatch,
                        "pending_tool binding must be a ToolCall",
                    )
                });
            match call {
                Ok(call) => {
                    world
                        .get_resource_mut::<MclDriverMailboxes>()
                        .expect("MclPlugin is installed")
                        .pending_tools
                        .entry(command.agent)
                        .or_default()
                        .push(call);
                    Some(Ok(MclCommandValue::Unit))
                }
                Err(error) => Some(Err(error)),
            }
        } else if command_text == "DELETE pending_tool FROM msg WHERE id == ?" {
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
        } else if command_text == "SELECT pending_tool FROM msg" {
            let values = world
                .get_resource::<MclDriverMailboxes>()
                .and_then(|mailboxes| mailboxes.pending_tools.get(&command.agent))
                .cloned()
                .unwrap_or_default();
            Some(Ok(MclCommandValue::Json(
                serde_json::to_value(values).expect("ToolCall is serializable"),
            )))
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
            state.conversation.extend(request.restored_messages.clone());
        }
        // Lua Base Driver is the authoritative control loop. It starts after
        // AgentMcl exists so command transactions can address this Agent.
        if is_lua_driver {
            crate::spawn_base_driver(agent, driver_source, self.event_sender())
                .map_err(|error| MclError::new(MclErrorKind::LuaFailed, error))?;
        }
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
            if request_name != "req"
                || !state.message_block_created
                || !state.tool_block_created
                || !state.request_block_created
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
                .filter_map(|message| match message {
                    Message::System { content } => Some(content.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
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
                messages: state.conversation.clone(),
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
