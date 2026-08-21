use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_plugin::{Agent, AgentResourceEntry, AgentToolPending};
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{AgentFailure, AgentFailureKind, AgentMessage, Message, ResourceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    AgentMissing,
    ResourceMapMissing,
    InvalidResource,
    ResourceUnavailable,
    RegistrationFailed,
    ToolCallMissing,
    InvalidDefinition,
    ProviderMissing,
    ResourceResolutionFailed,
    AgentNotAlive,
    ToolEnvironmentMissing,
    ToolPluginMissing,
    ToolAlreadyRegistered,
    DuplicateResource,
    InvalidRequest,
    InvalidArguments,
    ExecutionFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        const LIMIT: usize = 512;
        let mut message = message.into();
        if message.len() > LIMIT {
            let mut boundary = LIMIT - 3;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
        }
        Self { kind, message }
    }
    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for ToolError {}

#[derive(Clone)]
pub struct AgentToolEnvironment {
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
}
impl AgentToolEnvironment {
    pub fn new(project_root: impl Into<PathBuf>, image_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Arc::new(project_root.into()),
            image_root: Arc::new(image_root.into()),
        }
    }
    pub fn project_root(&self) -> &Path {
        self.project_root.as_path()
    }
    pub fn image_root(&self) -> &Path {
        self.image_root.as_path()
    }
}
impl Component for AgentToolEnvironment {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolTemplate {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResourceContent {
    Prompt { role: String, content: Arc<str> },
}

impl ToolTemplate {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Result<Self, ToolError> {
        let template = Self {
            name: name.into(),
            description: description.into(),
            parameters,
        };
        validate_template(&template)?;
        Ok(template)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceMapEntry {
    pub resource_id: ResourceId,
    pub resource_name: String,
    pub alias: Option<String>,
    pub tool_id: Option<ResourceId>,
    pub template: Option<ToolTemplate>,
    pub content: Option<ResourceContent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResourceRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
}
impl Event for AgentResourceRegisterRequest {}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentResourceRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
    pub result: Result<ResourceMapEntry, ToolError>,
}
impl Event for AgentResourceRegisterResponse {}

pub use margatroid_types::ToolCallEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub turn_id: String,
    pub agent: Entity,
    pub tool_id: ResourceId,
    pub resource_id: ResourceId,
    pub tool_call_id: String,
    pub arguments: String,
}
impl Event for ToolCallRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallResponse {
    pub turn_id: String,
    pub agent: Entity,
    pub tool_call_id: String,
    pub result: Result<String, ToolError>,
}
impl Event for ToolCallResponse {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelToolTurn {
    pub turn_id: String,
    pub agent: Entity,
}
impl Event for CancelToolTurn {}

pub struct ToolPlugin {
    schedule: String,
}
impl ToolPlugin {
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
impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ToolPluginInstalled;
impl Resource for ToolPluginInstalled {}

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "RuntimePlugin is required",
            )
            .panic();
        }
        if app.world().contains_resource::<ToolPluginInstalled>() {
            ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                "ToolPlugin is already installed",
            )
            .panic();
        }
        if !app.contains_schedule(&self.schedule) {
            ToolError::new(
                ToolErrorKind::InvalidRequest,
                "ToolPlugin schedule does not exist",
            )
            .panic();
        }
        app.world_mut().insert_resource(ToolPluginInstalled);
        app.add_system(&self.schedule, tool_call_route_system)
            .add_system(&self.schedule, cancel_tool_turn_system)
            .add_system(&self.schedule, tool_call_response_system);
    }
}

pub fn register_agent_resource(
    world: &mut World,
    agent: Entity,
    entry: ResourceMapEntry,
) -> Result<ResourceMapEntry, ToolError> {
    let tool_id = entry.tool_id.clone().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "resource mapping is not executable",
        )
    })?;
    let template = entry.template.clone().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "executable resource has no tool template",
        )
    })?;
    let agent_state = world.get_component_mut::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    agent_state
        .resources
        .register_tool(AgentResourceEntry {
            resource_id: entry.resource_id.clone(),
            resource_name: entry.resource_name.clone(),
            tool_id,
            description: template.description,
            parameters: template.parameters,
        })
        .map_err(|error| ToolError::new(ToolErrorKind::DuplicateResource, error.to_string()))?;
    Ok(entry)
}

pub fn candidate_resource_entry(
    resource_id: ResourceId,
    alias: Option<String>,
    tool_id: ResourceId,
    mut template: ToolTemplate,
) -> Result<ResourceMapEntry, ToolError> {
    let resource_name = alias.clone().unwrap_or_else(|| resource_id.to_string());
    if resource_name.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "resource name is empty",
        ));
    }
    template.name = resource_name.clone();
    validate_template(&template)?;
    Ok(ResourceMapEntry {
        resource_id,
        resource_name,
        alias,
        tool_id: Some(tool_id),
        template: Some(template),
        content: None,
    })
}

/// Resolve the provider-neutral tool snapshot for one inference request.
/// Visibility is deliberately evaluated at the call site so a later import
/// or visibility change cannot mutate an already submitted request.
pub fn resolve_agent_tool_definitions(
    world: &World,
    agent: Entity,
    resources: &[ResourceId],
) -> Result<Vec<margatroid_types::ToolDefinition>, ToolError> {
    if resources.is_empty() {
        return Ok(Vec::new());
    }
    let agent_state = world.get_component::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    let mut definitions = Vec::with_capacity(resources.len());
    for resource_id in resources {
        let entries = agent_state.resources.tools_by_resource(resource_id);
        if entries.len() != 1 {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "visible resource is not registered exactly once",
            ));
        }
        let entry = entries[0];
        definitions.push(margatroid_types::ToolDefinition {
            name: entry.resource_name.clone(),
            description: entry.description.clone(),
            input_schema: entry.parameters.clone(),
        });
    }
    Ok(definitions)
}

pub fn validate_agent_tool_calls(
    world: &World,
    agent: Entity,
    calls: &[margatroid_types::ToolCall],
) -> Result<(), ToolError> {
    let agent_state = world.get_component::<Agent>(agent).ok_or_else(|| {
        ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
    })?;
    for call in calls {
        if agent_state
            .resources
            .tool_by_name(&call.tool_name)
            .is_none()
        {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "tool call is not registered for this Agent",
            ));
        }
    }
    Ok(())
}

fn cancel_tool_turn_system(world: &mut World) {
    let cancellations = world
        .event_reader::<CancelToolTurn>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for cancellation in cancellations {
        if let Some(agent) = world.get_component_mut::<Agent>(cancellation.agent) {
            agent.tools.pending.retain(|(entity, turn_id, _), _| {
                *entity != cancellation.agent || turn_id != &cancellation.turn_id
            });
        }
    }
}

fn tool_call_route_system(world: &mut World) {
    let calls = world
        .event_reader::<ToolCallEvent>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in calls {
        let result = (|| {
            if event.turn_id.is_empty() || event.call.id.is_empty() || !world.is_alive(event.agent)
            {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call event is invalid",
                ));
            }
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                &event.call.arguments,
            )
            .map_err(|_| {
                ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "tool arguments must be a JSON object",
                )
            })?;
            let entry = world
                .get_component::<Agent>(event.agent)
                .and_then(|agent| agent.resources.tool_by_name(&event.call.tool_name))
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidRequest,
                        "tool name is not registered for this Agent",
                    )
                })?;
            let request = ToolCallRequest {
                turn_id: event.turn_id.clone(),
                agent: event.agent,
                tool_id: entry.tool_id.clone(),
                resource_id: entry.resource_id.clone(),
                tool_call_id: event.call.id.clone(),
                arguments: event.call.arguments.clone(),
            };
            let agent = world
                .get_component_mut::<Agent>(event.agent)
                .ok_or_else(|| {
                    ToolError::new(ToolErrorKind::AgentNotAlive, "Agent component is missing")
                })?;
            let key = (
                event.agent,
                request.turn_id.clone(),
                request.tool_call_id.clone(),
            );
            if agent.tools.pending.contains_key(&key) {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call is already pending",
                ));
            }
            agent.tools.pending.insert(
                key,
                AgentToolPending {
                    turn_id: request.turn_id.clone(),
                    tool_call_id: request.tool_call_id.clone(),
                    resource_id: request.resource_id.clone(),
                    tool_id: request.tool_id.clone(),
                },
            );
            world.send_event(request);
            Ok::<(), ToolError>(())
        })();
        if let Err(error) = result {
            world.send_event(AgentFailure {
                id: event.turn_id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.to_string(),
            });
        }
    }
}

fn tool_call_response_system(world: &mut World) {
    let responses = world
        .event_reader::<ToolCallResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for response in responses {
        let request = world
            .get_component_mut::<Agent>(response.agent)
            .and_then(|agent| {
                agent.tools.pending.remove(&(
                    response.agent,
                    response.turn_id.clone(),
                    response.tool_call_id.clone(),
                ))
            });
        let Some(request) = request else {
            continue;
        };
        let content = response.result.unwrap_or_else(|error| error.to_string());
        world.send_event(AgentMessage {
            id: response.turn_id.clone(),
            agent: response.agent,
            message: Message::Tool {
                resource_id: request.resource_id,
                tool_call_id: response.tool_call_id,
                content,
            },
            usage: None,
        });
    }
}

fn validate_template(template: &ToolTemplate) -> Result<(), ToolError> {
    if template.description.trim().is_empty() || !template.parameters.is_object() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "tool template is invalid",
        ));
    }
    Ok(())
}
