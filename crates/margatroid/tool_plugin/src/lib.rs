use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, Message, ResourceId, ToolCall,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    InvalidDefinition,
    ProviderMissing,
    ResourceResolutionFailed,
    AgentNotAlive,
    ToolEnvironmentMissing,
    ToolPluginMissing,
    ToolAlreadyRegistered,
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
pub struct ToolMap {
    pub tool_name: String,
    pub tool_id: ResourceId,
    pub resource_id: ResourceId,
    pub template: ToolTemplate,
}

#[derive(Default)]
pub struct AgentToolMap {
    next_index: u64,
    tools: Vec<ToolMap>,
}

impl AgentToolMap {
    pub fn get_by_name(&self, tool_name: &str) -> Option<&ToolMap> {
        self.tools.iter().find(|map| map.tool_name == tool_name)
    }

    pub fn get_by_tool(&self, tool_id: &ResourceId) -> Vec<&ToolMap> {
        self.tools
            .iter()
            .filter(|map| &map.tool_id == tool_id)
            .collect()
    }

    pub fn get_by_resource(&self, resource_id: &ResourceId) -> Vec<&ToolMap> {
        self.tools
            .iter()
            .filter(|map| &map.resource_id == resource_id)
            .collect()
    }

    pub fn register(
        &mut self,
        tool_id: ResourceId,
        resource_id: ResourceId,
        mut template: ToolTemplate,
    ) -> Result<&ToolMap, ToolError> {
        if !self.get_by_resource(&resource_id).is_empty() {
            return Err(ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                "resource is already registered for this Agent",
            ));
        }
        let tool_name = generated_tool_name(
            resource_id.resource_type(),
            self.next_index,
            resource_id.name(),
        );
        self.next_index = self.next_index.checked_add(1).ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "Agent tool index is exhausted",
            )
        })?;
        template.name = tool_name.clone();
        validate_template(&template)?;
        self.tools.push(ToolMap {
            tool_name,
            tool_id,
            resource_id,
            template,
        });
        Ok(self.tools.last().expect("ToolMap was just inserted"))
    }
}
impl Component for AgentToolMap {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for AgentToolRegisterRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub result: Result<(), ToolError>,
}
impl Event for AgentToolRegisterResponse {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEvent {
    pub turn_id: String,
    pub agent: Entity,
    pub call: ToolCall,
}
impl Event for ToolCallEvent {}

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
pub struct ToolTurnCompleted {
    pub turn_id: String,
    pub agent: Entity,
}
impl Event for ToolTurnCompleted {}

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
        app.world_mut().insert_resource(PendingToolCalls::default());
        app.add_system(&self.schedule, tool_call_route_system)
            .add_system(&self.schedule, tool_call_response_system);
    }
}

pub fn attach_agent_tool_map(world: &mut World, agent: Entity) -> Result<(), ToolError> {
    if !world.is_alive(agent) {
        return Err(ToolError::new(
            ToolErrorKind::AgentNotAlive,
            "Agent entity is not alive",
        ));
    }
    if world.get_component::<AgentToolMap>(agent).is_some()
        || !world.insert_component(agent, AgentToolMap::default())
    {
        return Err(ToolError::new(
            ToolErrorKind::ToolAlreadyRegistered,
            "AgentToolMap is already attached",
        ));
    }
    Ok(())
}

pub fn register_agent_tool(
    world: &mut World,
    agent: Entity,
    tool_id: ResourceId,
    resource_id: ResourceId,
    template: ToolTemplate,
) -> Result<ToolMap, ToolError> {
    let map = world
        .get_component_mut::<AgentToolMap>(agent)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "AgentToolMap is not attached",
            )
        })?
        .register(tool_id, resource_id, template)?
        .clone();
    Ok(map)
}

#[derive(Default)]
struct PendingToolCalls {
    calls: Vec<ToolCallRequest>,
}

impl PendingToolCalls {
    fn get(&self, agent: Entity, turn_id: &str, tool_call_id: &str) -> Option<&ToolCallRequest> {
        self.calls.iter().find(|request| {
            request.agent == agent
                && request.turn_id == turn_id
                && request.tool_call_id == tool_call_id
        })
    }

    fn get_by_turn(&self, agent: Entity, turn_id: &str) -> Vec<&ToolCallRequest> {
        self.calls
            .iter()
            .filter(|request| request.agent == agent && request.turn_id == turn_id)
            .collect()
    }

    fn add_pending(&mut self, request: ToolCallRequest) -> Result<(), ToolError> {
        if self
            .get(request.agent, &request.turn_id, &request.tool_call_id)
            .is_some()
        {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "tool call is already pending",
            ));
        }
        self.calls.push(request);
        Ok(())
    }

    fn remove(
        &mut self,
        agent: Entity,
        turn_id: &str,
        tool_call_id: &str,
    ) -> Option<ToolCallRequest> {
        let index = self.calls.iter().position(|request| {
            request.agent == agent
                && request.turn_id == turn_id
                && request.tool_call_id == tool_call_id
        })?;
        Some(self.calls.remove(index))
    }
}
impl Resource for PendingToolCalls {}

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
            let map = world
                .get_component::<AgentToolMap>(event.agent)
                .and_then(|maps| maps.get_by_name(&event.call.tool_name))
                .cloned()
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidRequest,
                        "tool name is not registered for this Agent",
                    )
                })?;
            let request = ToolCallRequest {
                turn_id: event.turn_id.clone(),
                agent: event.agent,
                tool_id: map.tool_id,
                resource_id: map.resource_id,
                tool_call_id: event.call.id.clone(),
                arguments: event.call.arguments.clone(),
            };
            world
                .get_resource_mut::<PendingToolCalls>()
                .expect("ToolPlugin is installed")
                .add_pending(request.clone())?;
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
            .get_resource_mut::<PendingToolCalls>()
            .expect("ToolPlugin is installed")
            .remove(response.agent, &response.turn_id, &response.tool_call_id);
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
        });
        let completed = world
            .get_resource::<PendingToolCalls>()
            .expect("ToolPlugin is installed")
            .get_by_turn(response.agent, &response.turn_id)
            .is_empty();
        if completed {
            world.send_event(ToolTurnCompleted {
                turn_id: response.turn_id,
                agent: response.agent,
            });
        }
    }
}

fn generated_tool_name(resource_type: &str, index: u64, resource_name: &str) -> String {
    let mut name = format!("{resource_type}{index}_");
    name.extend(resource_name.chars().map(|character| {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            character
        } else {
            '_'
        }
    }));
    name.truncate(64);
    name
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
