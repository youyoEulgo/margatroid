use std::collections::{BTreeMap, BTreeSet};

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::InferenceCommand;
use margatroid_types::{
    AgentContextMessagesUpdated, AgentFailure, AgentFailureKind, AgentMessage, Message,
    MessageIntent, ResourceRef, ToolCall, ToolDefinition,
};
use memory_plugin::{AgentMemoryWriteFailed, MemoryError, WorldMemoryExt};
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

struct AgentPluginInstalled;

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
            .add_system(&self.schedule, agent_message_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateRequest {
    pub id: String,
    pub agent_id: String,
    pub workspace_id: Entity,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub default_visibility: BTreeSet<ResourceRef>,
}

impl Event for AgentCreateRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreated {
    pub id: String,
    pub agent: Entity,
}

impl Event for AgentCreated {}

pub struct AgentIdentity {
    id: String,
}

impl AgentIdentity {
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl Component for AgentIdentity {}

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
}

impl AgentContext {
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender) {
        assert_dynamic_message(&message);
        self.messages.push(message);
        events.send_event(AgentContextMessagesUpdated {
            agent,
            messages: self.messages.clone(),
        });
    }

    pub fn rewrite_messages(
        &mut self,
        agent: Entity,
        messages: Vec<Message>,
        events: &RuntimeEventSender,
    ) {
        assert_dynamic_messages(&messages);
        self.messages = messages;
        events.send_event(AgentContextMessagesUpdated {
            agent,
            messages: self.messages.clone(),
        });
    }
}

impl Component for AgentContext {}

pub struct AgentDefaultVisibility {
    resources: BTreeSet<ResourceRef>,
}

impl AgentDefaultVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceRef> {
        &self.resources
    }
}

impl Component for AgentDefaultVisibility {}

pub struct AgentDynamicVisibility {
    resources: BTreeSet<ResourceRef>,
}

impl AgentDynamicVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceRef> {
        &self.resources
    }
}

impl Component for AgentDynamicVisibility {}

#[derive(Default)]
pub struct AgentStatus {
    pending_tools: Option<PendingToolCalls>,
}

impl AgentStatus {
    pub fn is_waiting_for_tools(&self) -> bool {
        self.pending_tools.is_some()
    }

    pub fn pending_turn_id(&self) -> Option<&str> {
        self.pending_tools
            .as_ref()
            .map(|pending| pending.id.as_str())
    }

    pub fn pending_tool_call_ids(&self) -> impl Iterator<Item = &str> + '_ {
        self.pending_tools
            .iter()
            .flat_map(|pending| pending.tool_call_ids.iter().map(String::as_str))
    }

    fn begin_tool_calls(
        &mut self,
        id: &str,
        tool_calls: &[ToolCall],
    ) -> Result<(), AgentStepError> {
        if self.pending_tools.is_some() || id.is_empty() || tool_calls.is_empty() {
            return Err(AgentStepError::InvalidToolBatch);
        }
        let mut tool_call_ids = BTreeSet::new();
        for tool_call in tool_calls {
            if tool_call.id.is_empty() || !tool_call_ids.insert(tool_call.id.clone()) {
                return Err(AgentStepError::InvalidToolBatch);
            }
        }
        self.pending_tools = Some(PendingToolCalls {
            id: id.to_owned(),
            tool_call_ids,
        });
        Ok(())
    }

    fn accepts_tool_response(&self, id: &str, tool_call_id: &str) -> bool {
        self.pending_tools
            .as_ref()
            .is_some_and(|pending| pending.id == id && pending.tool_call_ids.contains(tool_call_id))
    }

    fn complete_tool_response(&mut self, id: &str, tool_call_id: &str) -> bool {
        let Some(pending) = self.pending_tools.as_mut() else {
            return false;
        };
        if pending.id != id || !pending.tool_call_ids.remove(tool_call_id) {
            return false;
        }
        if pending.tool_call_ids.is_empty() {
            self.pending_tools = None;
            true
        } else {
            false
        }
    }
}

impl Component for AgentStatus {}

struct PendingToolCalls {
    id: String,
    tool_call_ids: BTreeSet<String>,
}

struct AvailableTools {
    definitions: Vec<ToolDefinition>,
    resources_by_name: BTreeMap<String, ResourceRef>,
}

enum AgentStepError {
    Memory(MemoryError),
    AgentMissing,
    ContextMissing,
    StatusMissing,
    InvalidMessage,
    InvalidToolBatch,
    Tool(ToolError),
    DuplicateToolName,
}

impl AgentStepError {
    fn failure_message(&self) -> String {
        match self {
            Self::Memory(error) => error.to_string(),
            Self::AgentMissing => "AgentMissing: agent entity is not alive".into(),
            Self::ContextMissing => "ContextMissing: agent context is missing".into(),
            Self::StatusMissing => "StatusMissing: agent status is missing".into(),
            Self::InvalidMessage => "InvalidMessage: message and intent do not match".into(),
            Self::InvalidToolBatch => "InvalidToolBatch: tool call batch is invalid".into(),
            Self::Tool(error) => error.to_string(),
            Self::DuplicateToolName => {
                "DuplicateToolName: visible resources expose the same tool name".into()
            }
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
            || request.agent_id.is_empty()
            || !world.is_alive(request.workspace_id)
            || request
                .messages
                .iter()
                .any(|message| matches!(message, Message::System { .. }))
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

fn agent_message_system(world: &mut World) {
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
        match handle_message(world, agent, &message, &events) {
            Ok(()) => {}
            Err(AgentStepError::Memory(error)) => {
                events.send_event(AgentMemoryWriteFailed { agent, error });
            }
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

fn handle_message(
    world: &mut World,
    agent: Entity,
    event: &AgentMessage,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    validate_message_intent(event)?;
    match (&event.intent, &event.message) {
        (MessageIntent::ResolveToolCall, Message::Tool { tool_call_id, .. }) => {
            let accepts_response = world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .accepts_tool_response(&event.id, tool_call_id);
            if !accepts_response {
                return Err(AgentStepError::InvalidToolBatch);
            }
            record_message(world, agent, event, events)?;
            let is_last_response = world
                .get_component_mut::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .complete_tool_response(&event.id, tool_call_id);
            if is_last_response {
                send_inference_command(world, &event.id, agent, events)?;
            }
        }
        (MessageIntent::CompleteTurn, Message::Assistant { .. }) => {
            record_message(world, agent, event, events)?;
        }
        (MessageIntent::UserWithToolCalls { tool_calls }, Message::User { .. }) => {
            record_message(world, agent, event, events)?;
            dispatch_tool_calls(world, &event.id, agent, tool_calls, events)?;
        }
        (MessageIntent::DispatchToolCalls, Message::Assistant { tool_calls, .. }) => {
            record_message(world, agent, event, events)?;
            dispatch_tool_calls(world, &event.id, agent, tool_calls, events)?;
        }
        (MessageIntent::UserWithoutToolCalls, Message::User { .. }) => {
            record_message(world, agent, event, events)?;
            send_inference_command(world, &event.id, agent, events)?;
        }
        _ => return Err(AgentStepError::InvalidMessage),
    }
    Ok(())
}

fn record_message(
    world: &mut World,
    agent: Entity,
    event: &AgentMessage,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    if !world.is_alive(agent) {
        return Err(AgentStepError::AgentMissing);
    }
    if world.get_component::<AgentContext>(agent).is_none() {
        return Err(AgentStepError::ContextMissing);
    }
    match event.message {
        Message::User { .. } | Message::Assistant { .. } => world
            .append_history_message(event)
            .map_err(AgentStepError::Memory)?,
        Message::Tool { .. } => {}
        Message::System { .. } => return Err(AgentStepError::InvalidMessage),
    }
    world
        .get_component_mut::<AgentContext>(agent)
        .expect("AgentContext existence was checked")
        .append_message(agent, event.message.clone(), events);
    Ok(())
}

fn dispatch_tool_calls(
    world: &mut World,
    id: &str,
    agent: Entity,
    tool_calls: &[ToolCall],
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let available_tools = build_available_tools(world, agent)?;
    let requests = tool_calls
        .iter()
        .map(|call| {
            let resource = available_tools
                .resources_by_name
                .get(&call.name)
                .ok_or_else(|| {
                    AgentStepError::Tool(ToolError::new(
                        ToolErrorKind::InvalidRequest,
                        "tool call name was not present in current tool definitions",
                    ))
                })?;
            Ok(ToolCallRequest {
                id: id.to_owned(),
                agent,
                resource: resource.clone(),
                call: call.clone(),
            })
        })
        .collect::<Result<Vec<_>, AgentStepError>>()?;
    world
        .get_component_mut::<AgentStatus>(agent)
        .ok_or(AgentStepError::StatusMissing)?
        .begin_tool_calls(id, tool_calls)?;
    let agent_id = world
        .get_component::<AgentIdentity>(agent)
        .map(AgentIdentity::id)
        .unwrap_or("unknown");
    tracing::info!(
        request_id = id,
        agent = agent_id,
        tool_calls = requests.len(),
        "tool calls dispatched"
    );
    for request in requests {
        events.send_event(request);
    }
    Ok(())
}

fn send_inference_command(
    world: &World,
    id: &str,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let available_tools = build_available_tools(world, agent)?;
    let context = world
        .get_component::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?;
    let mut messages = Vec::with_capacity(context.messages.len() + 1);
    messages.push(Message::System {
        content: context.system_prompt.clone(),
    });
    messages.extend(context.messages.iter().cloned());
    let agent_id = world
        .get_component::<AgentIdentity>(agent)
        .map(AgentIdentity::id)
        .unwrap_or("unknown");
    tracing::info!(
        request_id = id,
        agent = agent_id,
        messages = messages.len(),
        tools = available_tools.definitions.len(),
        "inference requested"
    );
    events.send_event(InferenceCommand {
        id: id.to_owned(),
        agent,
        messages,
        tools: available_tools.definitions,
        stream: None,
    });
    Ok(())
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
    let mut resources_by_name = BTreeMap::new();
    for resource in resources {
        let tool = world
            .resolve_tool(agent, &resource)
            .map_err(AgentStepError::Tool)?;
        if resources_by_name
            .insert(tool.definition().name.clone(), resource)
            .is_some()
        {
            return Err(AgentStepError::DuplicateToolName);
        }
        definitions.push(tool.definition().clone());
    }
    Ok(AvailableTools {
        definitions,
        resources_by_name,
    })
}

fn validate_message_intent(event: &AgentMessage) -> Result<(), AgentStepError> {
    match (&event.intent, &event.message) {
        (MessageIntent::UserWithToolCalls { .. }, Message::User { .. })
        | (MessageIntent::UserWithoutToolCalls, Message::User { .. })
        | (MessageIntent::DispatchToolCalls, Message::Assistant { .. })
        | (MessageIntent::ResolveToolCall, Message::Tool { .. })
        | (MessageIntent::CompleteTurn, Message::Assistant { .. }) => Ok(()),
        _ => Err(AgentStepError::InvalidMessage),
    }
}

fn assert_dynamic_message(message: &Message) {
    assert!(
        !matches!(message, Message::System { .. }),
        "AgentContext dynamic messages cannot contain System messages"
    );
}

fn assert_dynamic_messages(messages: &[Message]) {
    for message in messages {
        assert_dynamic_message(message);
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use async_runtime_plugin::AsyncRuntimePlugin;
    use margatroid_types::{ResourceName, ToolDefinition};
    use memory_plugin::{AgentMemory, MemoryPlugin};
    use serde::Deserialize;
    use serde_json::json;
    use tempfile::tempdir;
    use tool_plugin::{AgentToolEnvironment, AppToolExt, Tool, ToolContext, ToolPlugin};

    use super::*;

    #[derive(Deserialize)]
    struct EmptyArguments {}

    fn resource(name: &str) -> ResourceRef {
        ResourceRef::new("tool", ResourceName::new(name).unwrap()).unwrap()
    }

    fn tool(resource: ResourceRef, exposed_name: &str) -> Tool {
        Tool::new(
            resource,
            ToolDefinition {
                name: exposed_name.into(),
                description: "Test tool".into(),
                input_schema: json!({"type":"object"}),
            },
            |_context: ToolContext, _arguments: EmptyArguments| async move {
                Ok::<_, Infallible>("ok".into())
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

    fn create_agent(app: &mut App, visibility: BTreeSet<ResourceRef>) -> Entity {
        let workspace = app.world_mut().spawn();
        app.world().send_event(AgentCreateRequest {
            id: "agent-1".into(),
            agent_id: "test.agent0".into(),
            workspace_id: workspace,
            system_prompt: "system".into(),
            messages: Vec::new(),
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
    fn duplicate_exposed_names_are_rejected_from_a_request() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool(tool(first.clone(), "same"));
        app.register_tool(tool(second.clone(), "same"));
        let agent = create_agent(
            &mut app,
            [first, second].into_iter().collect::<BTreeSet<_>>(),
        );

        assert!(matches!(
            build_available_tools(app.world(), agent),
            Err(AgentStepError::DuplicateToolName)
        ));
    }

    #[test]
    fn unavailable_visible_resource_emits_agent_failure() {
        let mut app = test_app();
        let missing =
            ResourceRef::new("skill", ResourceName::new("local/missing").unwrap()).unwrap();
        let agent = create_agent(&mut app, [missing].into_iter().collect());
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "hello".into(),
            },
            intent: MessageIntent::UserWithoutToolCalls,
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
            },
            intent: MessageIntent::UserWithoutToolCalls,
        });
        app.tick();
        app.tick();

        let command = app
            .world()
            .event_reader::<InferenceCommand>()
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
            }]
        );
    }

    #[test]
    fn tool_responses_continue_only_after_the_last_call() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool(tool(first.clone(), "first"));
        app.register_tool(tool(second.clone(), "second"));
        let agent = create_agent(
            &mut app,
            [first, second].into_iter().collect::<BTreeSet<_>>(),
        );
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "use both".into(),
            },
            intent: MessageIntent::UserWithToolCalls {
                tool_calls: vec![
                    ToolCall {
                        id: "first-call".into(),
                        name: "first".into(),
                        arguments: "{}".into(),
                    },
                    ToolCall {
                        id: "second-call".into(),
                        name: "second".into(),
                        arguments: "{}".into(),
                    },
                ],
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut commands = 0;
        let mut settled_frames = 0;
        loop {
            app.tick();
            commands += app
                .world()
                .event_reader::<InferenceCommand>()
                .into_iter()
                .filter(|command| command.id == "turn-1")
                .count();
            let context = app.world().get_component::<AgentContext>(agent).unwrap();
            let status = app.world().get_component::<AgentStatus>(agent).unwrap();
            if commands == 1 && !status.is_waiting_for_tools() && context.messages().len() == 3 {
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
}
