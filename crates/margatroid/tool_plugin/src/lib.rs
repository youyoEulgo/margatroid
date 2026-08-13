use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{AgentMessage, Message, ResourceId, ToolCall, ToolDefinition};

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
    id: ResourceId,
    definition: ToolDefinition,
}
impl ToolTemplate {
    pub fn new(id: ResourceId, definition: ToolDefinition) -> Result<Self, ToolError> {
        if id.resource_type() != "tool" {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "tool template ID must use type tool",
            ));
        }
        validate_definition(&definition)?;
        Ok(Self { id, definition })
    }
    pub fn id(&self) -> &ResourceId {
        &self.id
    }
    pub fn definition(&self) -> &ToolDefinition {
        &self.definition
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub id: String,
    pub agent: Entity,
    pub call: ToolCall,
}
impl Event for ToolCallRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEvent {
    pub id: String,
    pub agent: Entity,
    pub loader: ResourceId,
    pub resource: ResourceId,
    pub call: ToolCall,
}
impl Event for ToolCallEvent {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDefinitionRequest {
    pub id: String,
    pub agent: Entity,
    pub resource: ResourceId,
}
impl Event for ToolDefinitionRequest {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinitionRoute {
    pub id: String,
    pub agent: Entity,
    pub loader: ResourceId,
    pub resource: ResourceId,
}
impl Event for ToolDefinitionRoute {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinitionResult {
    pub id: String,
    pub agent: Entity,
    pub resource: ResourceId,
    pub result: Result<ToolDefinition, ToolError>,
}
impl Event for ToolDefinitionResult {}

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
        app.world_mut().insert_resource(ToolRegistry::default());
        app.add_system(&self.schedule, tool_definition_route_system)
            .add_system(&self.schedule, tool_call_route_system);
    }
}

pub trait AppToolExt {
    fn register_tool_template(&mut self, template: ToolTemplate) -> &mut Self;
}
impl AppToolExt for App {
    fn register_tool_template(&mut self, template: ToolTemplate) -> &mut Self {
        let registry = self
            .world_mut()
            .get_resource_mut::<ToolRegistry>()
            .unwrap_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolPluginMissing,
                    "ToolPlugin is not installed",
                )
                .panic()
            });
        registry
            .insert(template)
            .unwrap_or_else(|error| error.panic());
        self
    }
}

pub trait WorldToolExt {
    fn tool_template(&self, id: &ResourceId) -> Option<ToolTemplate>;
    fn tool_definition_for(&self, resource: &ResourceId) -> Option<ToolDefinition>;
}
impl WorldToolExt for World {
    fn tool_template(&self, id: &ResourceId) -> Option<ToolTemplate> {
        self.get_resource::<ToolRegistry>()?
            .templates
            .get(id)
            .cloned()
    }
    fn tool_definition_for(&self, resource: &ResourceId) -> Option<ToolDefinition> {
        let id = match resource.resource_type() {
            "tool" => resource.clone(),
            "skill" => loader_id("skill")?,
            _ => return None,
        };
        let mut definition = self.tool_template(&id)?.definition().clone();
        definition.name = resource.to_string();
        if resource.resource_type() != "tool" {
            definition.input_schema = serde_json::json!({"type":"object"});
        }
        Some(definition)
    }
}

#[derive(Default)]
struct ToolRegistry {
    templates: BTreeMap<ResourceId, ToolTemplate>,
}
impl ToolRegistry {
    fn insert(&mut self, template: ToolTemplate) -> Result<(), ToolError> {
        if self.templates.contains_key(template.id()) {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "tool template is already registered",
            ));
        }
        self.templates.insert(template.id().clone(), template);
        Ok(())
    }
}
impl Resource for ToolRegistry {}

fn loader_id(resource_type: &str) -> Option<ResourceId> {
    ResourceId::parse(format!("tool:builtin/{resource_type}-loader:latest")).ok()
}

fn tool_definition_route_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolDefinitionRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let loader = loader_id(request.resource.resource_type());
        if let Some(loader) = loader.filter(|id| world.tool_template(id).is_some()) {
            world.send_event(ToolDefinitionRoute {
                id: request.id,
                agent: request.agent,
                loader,
                resource: request.resource,
            });
        } else {
            world.send_event(ToolDefinitionResult {
                id: request.id,
                agent: request.agent,
                resource: request.resource,
                result: Err(ToolError::new(
                    ToolErrorKind::ProviderMissing,
                    "resource loader is not registered",
                )),
            });
        }
    }
}

fn tool_call_route_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let resource = request.call.resource.clone();
        let loader = match resource.resource_type() {
            "tool" => resource.clone(),
            "skill" => match loader_id("skill") {
                Some(loader) => loader,
                None => {
                    send_error(world, request, "resource loader ID is invalid");
                    continue;
                }
            },
            _ => {
                send_error(world, request, "resource type is not callable");
                continue;
            }
        };
        if world.tool_template(&loader).is_none() {
            send_error(world, request, "resource loader is not registered");
            continue;
        }
        let call = if resource.resource_type() == "skill" {
            ToolCall {
                id: request.call.id.clone(),
                resource: resource.clone(),
                arguments: serde_json::json!({"resource": resource.to_string()}).to_string(),
            }
        } else {
            request.call
        };
        world.send_event(ToolCallEvent {
            id: request.id,
            agent: request.agent,
            loader,
            resource,
            call,
        });
    }
}

fn send_error(world: &World, request: ToolCallRequest, content: &str) {
    world.send_event(AgentMessage {
        id: request.id,
        agent: request.agent,
        message: Message::Tool {
            tool_call_id: request.call.id,
            content: content.to_owned(),
        },
    });
}

fn validate_definition(definition: &ToolDefinition) -> Result<(), ToolError> {
    let valid_name = !definition.name.is_empty()
        && definition.name.len() <= 64
        && definition
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if !valid_name
        || definition.description.trim().is_empty()
        || !definition.input_schema.is_object()
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "tool template definition is invalid",
        ));
    }
    Ok(())
}
