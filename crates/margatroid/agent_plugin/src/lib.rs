use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::InferenceRequest;
use margatroid_types::{
    AgentContextMessagesUpdated, AgentFailure, AgentFailureKind, AgentHistoryMessageWriteRequested,
    AgentMessage, Message, ResourceId, ToolCall, ToolDefinition,
};
use tool_plugin::{ToolCallRequest, ToolError, ToolErrorKind, WorldToolExt};

pub struct AgentPlugin {
    schedule: String,
}

impl AgentPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for AgentPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AgentPluginInstalled;

impl Resource for AgentPluginInstalled {}

impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("AgentPlugin requires RuntimePlugin");
        }
        if app.world().contains_resource::<AgentPluginInstalled>() {
            panic!("AgentPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("AgentPlugin schedule does not exist");
        }

        app.world_mut().insert_resource(AgentPluginInstalled);
        app.add_system(&self.schedule, agent_create_system)
            .add_system(&self.schedule, agent_tool_call_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateRequest {
    pub id: String,
    pub agent_id: ResourceId,
    pub workspace_id: Entity,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tool_context: Vec<Message>,
    pub default_visibility: BTreeSet<ResourceId>,
}

impl Event for AgentCreateRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreated {
    pub id: String,
    pub agent: Entity,
}

impl Event for AgentCreated {}

pub struct AgentIdentity {
    id: ResourceId,
}

impl AgentIdentity {
    pub fn id(&self) -> &ResourceId {
        &self.id
    }
}

impl Component for AgentIdentity {}

pub trait WorldAgentExt {
    fn agent(&self, id: &ResourceId) -> Option<Entity>;
}

impl WorldAgentExt for World {
    fn agent(&self, id: &ResourceId) -> Option<Entity> {
        self.query_with::<AgentIdentity>()
            .result()
            .into_iter()
            .find(|entity| {
                self.get_component::<AgentIdentity>(*entity)
                    .is_some_and(|identity| identity.id() == id)
            })
    }
}

pub struct AgentWorkspaceId {
    workspace_id: Entity,
}

impl AgentWorkspaceId {
    pub fn workspace_id(&self) -> Entity {
        self.workspace_id
    }
}

impl Component for AgentWorkspaceId {}

pub struct AgentContext {
    system_prompt: String,
    messages: Vec<Message>,
    tool_context: Vec<Message>,
}

impl AgentContext {
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tool_context(&self) -> &[Message] {
        &self.tool_context
    }

    pub fn append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender) {
        assert_conversation_message(&message);
        self.messages.push(message);
        self.notify_updated(agent, events);
    }

    pub fn rewrite_messages(
        &mut self,
        agent: Entity,
        messages: Vec<Message>,
        events: &RuntimeEventSender,
    ) {
        assert_conversation_messages(&messages);
        self.messages = messages;
        self.notify_updated(agent, events);
    }

    pub fn append_tool_context(
        &mut self,
        agent: Entity,
        message: Message,
        events: &RuntimeEventSender,
    ) {
        assert!(matches!(message, Message::Tool { .. }));
        self.tool_context.push(message);
        self.notify_updated(agent, events);
    }

    pub fn clear_tool_context(&mut self, agent: Entity, events: &RuntimeEventSender) {
        if self.tool_context.is_empty() {
            return;
        }
        self.tool_context.clear();
        self.notify_updated(agent, events);
    }

    fn notify_updated(&self, agent: Entity, events: &RuntimeEventSender) {
        events.send_event(AgentContextMessagesUpdated {
            agent,
            messages: self.messages.clone(),
            tool_context: self.tool_context.clone(),
        });
    }
}

impl Component for AgentContext {}

pub struct AgentDefaultVisibility {
    resources: BTreeSet<ResourceId>,
}

impl AgentDefaultVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceId> {
        &self.resources
    }
}

impl Component for AgentDefaultVisibility {}

pub struct AgentDynamicVisibility {
    resources: BTreeSet<ResourceId>,
}

impl AgentDynamicVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceId> {
        &self.resources
    }
}

impl Component for AgentDynamicVisibility {}

#[derive(Default)]
pub(crate) struct AgentStatus {
    pending_tools: BTreeMap<String, PendingToolCall>,
    loading_skills: BTreeMap<String, PendingToolCall>,
}

impl AgentStatus {
    pub(crate) fn add_tool_call(&mut self, call: PendingToolCall) -> Result<(), AgentStepError> {
        if call.call.id.is_empty() || self.pending_tools.contains_key(&call.call.id) {
            return Err(AgentStepError::InvalidToolBatch);
        }
        self.pending_tools.insert(call.call.id.clone(), call);
        Ok(())
    }

    pub(crate) fn complete_tool_call(&mut self, id: &str) -> ToolCallCompletion {
        if self.pending_tools.remove(id).is_none() {
            return ToolCallCompletion::Invalid;
        }
        if self.pending_tools.is_empty() {
            ToolCallCompletion::Completed
        } else {
            ToolCallCompletion::Pending
        }
    }

    pub(crate) fn load_skill(&mut self, call: PendingToolCall) -> Result<(), AgentStepError> {
        if call.kind != ToolCallKind::Skill {
            return Err(AgentStepError::InvalidToolBatch);
        }
        self.loading_skills.insert(skill_key(&call.resource), call);
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn unload_skill(&mut self, key: &str) -> bool {
        self.loading_skills.remove(key).is_some()
    }

    #[allow(dead_code)]
    pub(crate) fn unload_all_skills(&mut self) {
        self.loading_skills.clear();
    }
}

impl Component for AgentStatus {}

#[derive(Clone)]
pub(crate) struct PendingToolCall {
    pub(crate) call: ToolCall,
    pub(crate) resource: ResourceId,
    pub(crate) kind: ToolCallKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolCallKind {
    Tool,
    Skill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolCallCompletion {
    Invalid,
    Pending,
    Completed,
}

struct AvailableTools {
    definitions: Vec<ToolDefinition>,
    resources: BTreeSet<ResourceId>,
}

enum ConversationTurnResult {
    WaitForTools,
    FinishTurn,
    RequestInference,
}

enum AgentStepError {
    AgentMissing,
    IdentityMissing,
    ContextMissing,
    StatusMissing,
    InvalidMessage,
    InvalidToolBatch,
    Tool(ToolError),
}

impl AgentStepError {
    fn failure_message(&self) -> String {
        match self {
            Self::AgentMissing => "AgentMissing: agent entity is not alive".into(),
            Self::IdentityMissing => "IdentityMissing: agent identity is missing".into(),
            Self::ContextMissing => "ContextMissing: agent context is missing".into(),
            Self::StatusMissing => "StatusMissing: agent status is missing".into(),
            Self::InvalidMessage => "InvalidMessage: message type is invalid".into(),
            Self::InvalidToolBatch => "InvalidToolBatch: tool call batch is invalid".into(),
            Self::Tool(error) => error.to_string(),
        }
    }
}

fn agent_create_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentCreateRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for request in requests {
        if request.id.is_empty()
            || request.agent_id.resource_type() != "agent"
            || world.agent(&request.agent_id).is_some()
            || !world.is_alive(request.workspace_id)
            || request
                .messages
                .iter()
                .any(|message| !matches!(message, Message::User { .. } | Message::Assistant { .. }))
            || request
                .tool_context
                .iter()
                .any(|message| !matches!(message, Message::Tool { .. }))
        {
            continue;
        }
        let dynamic_visibility = request.default_visibility.clone();
        let agent = world.spawn();
        assert!(world.insert_component(
            agent,
            AgentIdentity {
                id: request.agent_id,
            },
        ));
        assert!(world.insert_component(
            agent,
            AgentWorkspaceId {
                workspace_id: request.workspace_id,
            },
        ));
        assert!(world.insert_component(
            agent,
            AgentContext {
                system_prompt: request.system_prompt,
                messages: request.messages,
                tool_context: request.tool_context,
            },
        ));
        assert!(world.insert_component(
            agent,
            AgentDefaultVisibility {
                resources: request.default_visibility,
            },
        ));
        assert!(world.insert_component(
            agent,
            AgentDynamicVisibility {
                resources: dynamic_visibility,
            },
        ));
        assert!(world.insert_component(agent, AgentStatus::default()));
        events.send_event(AgentCreated {
            id: request.id,
            agent,
        });
    }
}

fn agent_tool_call_system(world: &mut World) {
    let messages = world
        .event_reader::<AgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for message in messages {
        let agent = message.agent;
        if !world.is_alive(agent) {
            tracing::warn!(id = %message.id, "AgentMessage agent does not exist");
            continue;
        }
        match handle_agent_message(world, &message, &events) {
            Ok(_) => {}
            Err(error) => {
                events.send_event(AgentFailure {
                    id: message.id.clone(),
                    agent,
                    kind: AgentFailureKind::Agent,
                    message: error.failure_message(),
                });
            }
        }
    }
}

fn handle_agent_message(
    world: &mut World,
    event: &AgentMessage,
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let agent = event.agent;
    let result = match &event.message {
        Message::System { .. } => return Err(AgentStepError::InvalidMessage),
        Message::User { tool_calls, .. } => {
            clear_tool_context(world, agent, events)?;
            record_history_message(event, None, events);
            append_conversation_message(world, agent, event.message.clone(), events)?;
            dispatch_tool_calls(world, &event.id, agent, tool_calls, true, events)?
        }
        Message::Assistant { tool_calls, .. } => {
            clear_tool_context(world, agent, events)?;
            record_history_message(event, None, events);
            append_conversation_message(world, agent, event.message.clone(), events)?;
            if !tool_calls.is_empty() {
                dispatch_tool_calls(world, &event.id, agent, tool_calls, true, events)?
            } else {
                ConversationTurnResult::FinishTurn
            }
        }
        Message::Tool { tool_call_id, .. } => {
            let pending = world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .pending_tools
                .get(tool_call_id)
                .cloned()
                .ok_or(AgentStepError::InvalidToolBatch)?;
            record_history_message(event, Some(&pending), events);
            append_tool_context(world, agent, event.message.clone(), events)?;
            if pending.kind == ToolCallKind::Skill {
                world
                    .get_component_mut::<AgentStatus>(agent)
                    .ok_or(AgentStepError::StatusMissing)?
                    .load_skill(pending)?;
            }
            let completion = world
                .get_component_mut::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .complete_tool_call(tool_call_id);
            match completion {
                ToolCallCompletion::Invalid => return Err(AgentStepError::InvalidToolBatch),
                ToolCallCompletion::Pending => ConversationTurnResult::WaitForTools,
                ToolCallCompletion::Completed => ConversationTurnResult::RequestInference,
            }
        }
    };
    if matches!(result, ConversationTurnResult::RequestInference) {
        send_inference_command(world, &event.id, agent, events)?;
    }
    Ok(result)
}

fn record_history_message(
    event: &AgentMessage,
    pending: Option<&PendingToolCall>,
    events: &RuntimeEventSender,
) {
    let message = match (pending.map(|pending| pending.kind), &event.message) {
        (Some(ToolCallKind::Skill), Message::Tool { tool_call_id, .. }) => Message::Tool {
            tool_call_id: tool_call_id.clone(),
            content: format!(
                "skill: {} loaded",
                pending.expect("skill pending call exists").resource.name()
            ),
        },
        _ => event.message.clone(),
    };
    events.send_event(AgentHistoryMessageWriteRequested {
        id: event.id.clone(),
        agent: event.agent,
        message,
    });
}

fn append_conversation_message(
    world: &mut World,
    agent: Entity,
    message: Message,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    if !world.is_alive(agent) {
        return Err(AgentStepError::AgentMissing);
    }
    if world.get_component::<AgentContext>(agent).is_none() {
        return Err(AgentStepError::ContextMissing);
    }
    world
        .get_component_mut::<AgentContext>(agent)
        .expect("AgentContext existence was checked")
        .append_message(agent, message, events);
    Ok(())
}

fn append_tool_context(
    world: &mut World,
    agent: Entity,
    message: Message,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    world
        .get_component_mut::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .append_tool_context(agent, message, events);
    Ok(())
}

fn clear_tool_context(
    world: &mut World,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    world
        .get_component_mut::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .clear_tool_context(agent, events);
    Ok(())
}

fn dispatch_tool_calls(
    world: &mut World,
    id: &str,
    agent: Entity,
    tool_calls: &[ToolCall],
    include_loading_skills: bool,
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    if !world
        .get_component::<AgentStatus>(agent)
        .ok_or(AgentStepError::StatusMissing)?
        .pending_tools
        .is_empty()
    {
        return Err(AgentStepError::InvalidToolBatch);
    }
    let available_tools = build_available_tools(world, agent)?;
    let mut pending = queue_tool_calls(&available_tools, tool_calls)?;
    if include_loading_skills {
        pending.extend(expand_loading_skills(
            world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?,
        ));
    }
    if pending.is_empty() {
        return Ok(ConversationTurnResult::RequestInference);
    }
    let mut call_ids = BTreeSet::new();
    if pending
        .iter()
        .any(|pending| pending.call.id.is_empty() || !call_ids.insert(pending.call.id.clone()))
    {
        return Err(AgentStepError::InvalidToolBatch);
    }
    {
        let status = world
            .get_component_mut::<AgentStatus>(agent)
            .ok_or(AgentStepError::StatusMissing)?;
        for call in pending.iter().cloned() {
            status.add_tool_call(call)?;
        }
    }
    dispatch_pending_tools(world, id, agent, events)?;
    Ok(ConversationTurnResult::WaitForTools)
}

fn queue_tool_calls(
    available_tools: &AvailableTools,
    tool_calls: &[ToolCall],
) -> Result<Vec<PendingToolCall>, AgentStepError> {
    tool_calls
        .iter()
        .map(|call| {
            let resource = call.resource.clone();
            if !available_tools.resources.contains(&resource) {
                return Err(AgentStepError::Tool(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call resource was not present in current tool definitions",
                )));
            }
            if resource.resource_type().is_empty() {
                return Err(AgentStepError::Tool(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call resource is invalid",
                )));
            }
            Ok(PendingToolCall {
                call: call.clone(),
                kind: tool_call_kind(&resource),
                resource: resource.clone(),
            })
        })
        .collect()
}

fn dispatch_pending_tools(
    world: &World,
    id: &str,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let requests = world
        .get_component::<AgentStatus>(agent)
        .ok_or(AgentStepError::StatusMissing)?
        .pending_tools
        .values()
        .cloned()
        .map(|pending| ToolCallRequest {
            id: id.to_owned(),
            agent,
            call: pending.call,
        })
        .collect::<Vec<_>>();
    let agent_id = world
        .get_component::<AgentIdentity>(agent)
        .map(AgentIdentity::id)
        .map(ToString::to_string)
        .unwrap_or_else(|| "unknown".into());
    tracing::info!(
        request_id = id,
        agent = %agent_id,
        tool_calls = requests.len(),
        "tool calls dispatched"
    );
    for request in requests {
        events.send_event(request);
    }
    Ok(())
}

fn expand_loading_skills(status: &AgentStatus) -> Vec<PendingToolCall> {
    static NEXT_SKILL_CALL_ID: AtomicU64 = AtomicU64::new(1);

    status
        .loading_skills
        .values()
        .cloned()
        .map(|mut pending| {
            pending.call.id = format!(
                "loading-skill-{}",
                NEXT_SKILL_CALL_ID.fetch_add(1, Ordering::Relaxed)
            );
            pending
        })
        .collect()
}

fn tool_call_kind(resource: &ResourceId) -> ToolCallKind {
    if resource.resource_type() == "skill" {
        ToolCallKind::Skill
    } else {
        ToolCallKind::Tool
    }
}

fn skill_key(resource: &ResourceId) -> String {
    resource.to_string()
}

fn send_inference_command(
    world: &World,
    id: &str,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let available_tools = build_available_tools(world, agent)?;
    let messages = build_inference_context(world, agent)?;
    let agent_id = world
        .get_component::<AgentIdentity>(agent)
        .map(AgentIdentity::id)
        .ok_or(AgentStepError::IdentityMissing)?;
    tracing::info!(
        request_id = id,
        agent = %agent_id,
        messages = messages.len(),
        tools = available_tools.definitions.len(),
        "inference requested"
    );
    events.send_event(InferenceRequest {
        id: id.to_owned(),
        agent,
        agent_id: agent_id.clone(),
        messages,
        tools: available_tools.definitions,
    });
    Ok(())
}

fn build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError> {
    let context = world
        .get_component::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?;
    let mut messages = Vec::with_capacity(context.messages.len() + context.tool_context.len() + 1);
    messages.push(Message::System {
        content: context.system_prompt.clone(),
    });
    messages.extend(context.messages.iter().cloned());
    messages.extend(context.tool_context.iter().cloned());
    Ok(messages)
}

fn build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError> {
    let resources = world
        .get_component::<AgentDynamicVisibility>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .resources()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut definitions = Vec::with_capacity(resources.len());
    for resource in &resources {
        let definition = world.tool_definition_for(&resource).ok_or_else(|| {
            AgentStepError::Tool(ToolError::new(
                ToolErrorKind::ProviderMissing,
                "resource provider was not registered",
            ))
        })?;
        definitions.push(definition);
    }
    Ok(AvailableTools {
        definitions,
        resources: resources.into_iter().collect(),
    })
}

fn assert_conversation_message(message: &Message) {
    assert!(
        matches!(message, Message::User { .. } | Message::Assistant { .. }),
        "AgentContext conversation messages must be User or Assistant"
    );
}

fn assert_conversation_messages(messages: &[Message]) {
    for message in messages {
        assert_conversation_message(message);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use async_runtime_plugin::AsyncRuntimePlugin;
    use margatroid_types::ToolDefinition;
    use memory_plugin::{AgentMemory, MemoryPlugin, WorldMemoryExt};
    use serde_json::json;
    use tempfile::tempdir;
    use tool_plugin::{AgentToolEnvironment, AppToolExt, ToolPlugin, ToolTemplate};

    use super::*;

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("tool:{name}")).unwrap()
    }

    fn tool(resource: ResourceId, exposed_name: &str) -> ToolTemplate {
        ToolTemplate::new(
            resource,
            ToolDefinition {
                name: exposed_name.into(),
                description: "Test tool".into(),
                input_schema: json!({"type":"object"}),
            },
        )
        .unwrap()
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ToolPlugin::default())
            .add_plugin(MemoryPlugin::default())
            .add_plugin(AgentPlugin::default());
        app
    }

    fn create_agent(app: &mut App, visibility: BTreeSet<ResourceId>) -> Entity {
        let workspace = app.world_mut().spawn();
        app.world().send_event(AgentCreateRequest {
            id: "agent-1".into(),
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            workspace_id: workspace,
            system_prompt: "system".into(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            default_visibility: visibility,
        });
        app.tick();
        app.tick();
        let agent = app
            .world()
            .event_reader::<AgentCreated>()
            .into_iter()
            .next()
            .unwrap()
            .agent;
        let directory = tempdir().unwrap();
        let path = directory.keep().join("memory.sql");
        let (memory, restored) = AgentMemory::open(path).unwrap();
        app.world_mut()
            .bind_agent_memory(agent, memory, &restored)
            .unwrap();
        app.world_mut().insert_component(
            agent,
            AgentToolEnvironment::new(Path::new("/project"), Path::new("/image")),
        );
        agent
    }

    #[test]
    fn creation_emits_an_entity_and_copies_default_visibility() {
        let mut app = test_app();
        let resource = resource("builtin/echo");
        let agent = create_agent(&mut app, [resource.clone()].into_iter().collect());
        assert_eq!(
            app.world()
                .get_component::<AgentDefaultVisibility>(agent)
                .unwrap()
                .resources(),
            app.world()
                .get_component::<AgentDynamicVisibility>(agent)
                .unwrap()
                .resources()
        );
        assert_eq!(
            app.world()
                .get_component::<AgentContext>(agent)
                .unwrap()
                .system_prompt(),
            "system"
        );
    }

    #[test]
    fn resource_ids_are_used_as_tool_definition_names() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool_template(tool(first.clone(), "same"));
        app.register_tool_template(tool(second.clone(), "same"));
        let agent = create_agent(
            &mut app,
            [first.clone(), second.clone()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        );

        let available = match build_available_tools(app.world(), agent) {
            Ok(available) => available,
            Err(_) => panic!("available tool templates should be valid"),
        };
        assert_eq!(available.definitions.len(), 2);
        assert_eq!(available.definitions[0].name, first.to_string());
        assert_eq!(available.definitions[1].name, second.to_string());
    }

    #[test]
    fn unavailable_visible_resource_emits_agent_failure() {
        let mut app = test_app();
        let missing = ResourceId::parse("skill:local/missing").unwrap();
        let agent = create_agent(&mut app, [missing].into_iter().collect());
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
        });
        app.tick();
        app.tick();

        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.id, "turn-1");
        assert_eq!(failure.agent, agent);
        assert_eq!(failure.kind, AgentFailureKind::Agent);
        assert_eq!(
            failure.message,
            "ProviderMissing: resource provider was not registered"
        );
    }

    #[test]
    fn user_message_updates_context_and_sends_inference() {
        let mut app = test_app();
        let agent = create_agent(&mut app, BTreeSet::new());
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
        });
        app.tick();
        app.tick();

        let command = app
            .world()
            .event_reader::<InferenceRequest>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(command.id, "turn-1");
        assert_eq!(command.messages.len(), 2);
        assert!(command.tools.is_empty());
        assert_eq!(
            app.world()
                .get_component::<AgentContext>(agent)
                .unwrap()
                .messages(),
            [Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            }]
        );
    }

    #[test]
    fn tool_responses_continue_only_after_the_last_call() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool_template(tool(first.clone(), "first"));
        app.register_tool_template(tool(second.clone(), "second"));
        let agent = create_agent(
            &mut app,
            [first, second].into_iter().collect::<BTreeSet<_>>(),
        );
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "use both".into(),
                tool_calls: vec![
                    ToolCall {
                        id: "first-call".into(),
                        resource: ResourceId::parse("tool:builtin/first").unwrap(),
                        arguments: "{}".into(),
                    },
                    ToolCall {
                        id: "second-call".into(),
                        resource: ResourceId::parse("tool:builtin/second").unwrap(),
                        arguments: "{}".into(),
                    },
                ],
            },
        });

        app.tick();
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Tool {
                tool_call_id: "first-call".into(),
                content: "first".into(),
            },
        });
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Tool {
                tool_call_id: "second-call".into(),
                content: "second".into(),
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut commands = 0;
        let mut settled_frames = 0;
        loop {
            app.tick();
            commands += app
                .world()
                .event_reader::<InferenceRequest>()
                .into_iter()
                .filter(|command| command.id == "turn-1")
                .count();
            let context = app.world().get_component::<AgentContext>(agent).unwrap();
            let status = app.world().get_component::<AgentStatus>(agent).unwrap();
            if commands == 1
                && status.pending_tools.is_empty()
                && context.messages().len() == 1
                && context.tool_context().len() == 2
            {
                settled_frames += 1;
                if settled_frames == 4 {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "tool responses did not settle");
            std::thread::yield_now();
        }
        assert_eq!(commands, 1);
    }

    #[test]
    fn loading_skills_are_templates_with_fresh_call_ids() {
        let resource = ResourceId::parse("skill:local/review").unwrap();
        let mut status = AgentStatus::default();
        assert!(status
            .load_skill(PendingToolCall {
                call: ToolCall {
                    id: "initial-call".into(),
                    resource: ResourceId::parse("skill:local/review").unwrap(),
                    arguments: "{}".into(),
                },
                resource,
                kind: ToolCallKind::Skill,
            })
            .is_ok());

        let first = expand_loading_skills(&status);
        let second = expand_loading_skills(&status);
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].call.id, "initial-call");
        assert_ne!(first[0].call.id, second[0].call.id);
        assert_eq!(first[0].resource, second[0].resource);
        assert!(status.unload_skill("skill:local/review:latest"));
        status.unload_all_skills();
    }
}
