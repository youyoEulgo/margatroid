use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Entity, Event, Plugin, Resource, World};
use margatroid_types::ResourceId;
use serde_json::json;
use tool_plugin::{
    register_agent_tool, AgentToolEnvironment, ToolCallRequest, ToolCallResponse, ToolError,
    ToolErrorKind, ToolTemplate,
};

const PROVIDER_ID: &str = "skill";
const SKILL_FILE: &str = "SKILL.md";
const SKILL_LOADER_ID: &str = "tool:builtin/skill-loader:latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for SkillRegisterRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub result: Result<(), ToolError>,
}
impl Event for SkillRegisterResponse {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillErrorKind {
    InvalidRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillError {
    kind: SkillErrorKind,
    message: String,
}

impl SkillError {
    fn new(kind: SkillErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> SkillErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SkillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SkillError {}

pub struct SkillPlugin {
    home_root: Arc<PathBuf>,
}

struct SkillRoots {
    home_root: Arc<PathBuf>,
}
impl Resource for SkillRoots {}

impl SkillPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, SkillError> {
        let home_root = normalize_root(home_root.into()).ok_or_else(|| {
            SkillError::new(
                SkillErrorKind::InvalidRoot,
                "skill root must be absolute and cannot contain parent traversal",
            )
        })?;
        Ok(Self {
            home_root: Arc::new(home_root),
        })
    }
}

impl Plugin for SkillPlugin {
    fn build(self, app: &mut App) {
        app.world_mut().insert_resource(SkillRoots {
            home_root: self.home_root.clone(),
        });
        app.add_system(RuntimePlugin::UPDATE, skill_register_system)
            .add_system(RuntimePlugin::UPDATE, skill_tool_call_system);
    }
}

fn skill_register_system(world: &mut World) {
    let requests = world
        .event_reader::<SkillRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = world
            .get_component::<AgentToolEnvironment>(request.agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent tool environment is missing",
                )
            })
            .and_then(|environment| {
                validate_skill_resource(&request.resource_id)?;
                let roots = world
                    .get_resource::<SkillRoots>()
                    .expect("SkillPlugin is installed");
                find_skill_file(environment, &roots.home_root, &request.resource_id)?;
                ToolTemplate::new(
                    request.resource_id.to_string(),
                    "Load this skill resource.",
                    json!({"type":"object"}),
                )
            });
        let result = result.and_then(|template| {
            register_agent_tool(
                world,
                request.agent,
                ResourceId::parse(SKILL_LOADER_ID).expect("built-in Skill loader ID is valid"),
                request.resource_id.clone(),
                template,
            )
            .map(|_| ())
        });
        world.send_event(SkillRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            result,
        });
    }
}

fn skill_tool_call_system(world: &mut World) {
    let calls = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in calls
        .into_iter()
        .filter(|event| event.tool_id == ResourceId::parse(SKILL_LOADER_ID).unwrap())
    {
        let result = world
            .get_component::<AgentToolEnvironment>(event.agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent tool environment is missing",
                )
            })
            .and_then(|environment| {
                validate_skill_resource(&event.resource_id)?;
                let roots = world
                    .get_resource::<SkillRoots>()
                    .expect("SkillPlugin is installed");
                let path = find_skill_file(environment, &roots.home_root, &event.resource_id)?;
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                    &event.arguments,
                )
                .map_err(|_| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "skill arguments must be a JSON object",
                    )
                })?;
                fs::read_to_string(path).map_err(|_| {
                    ToolError::new(
                        ToolErrorKind::ResourceResolutionFailed,
                        "skill file could not be read",
                    )
                })
            });
        world.send_event(ToolCallResponse {
            turn_id: event.turn_id,
            agent: event.agent,
            tool_call_id: event.tool_call_id,
            result,
        });
    }
}

fn find_skill_file(
    environment: &AgentToolEnvironment,
    home_root: &Path,
    resource: &ResourceId,
) -> Result<PathBuf, ToolError> {
    let candidates = [
        environment
            .project_root()
            .join(".margatroid")
            .join("skills"),
        environment.image_root().join("skills"),
        home_root.to_path_buf(),
    ];
    for root in candidates {
        let package = root
            .join(resource.scope())
            .join(resource.name())
            .join(resource.tag());
        let metadata = match fs::metadata(&package) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "skill package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "skill package is not a directory",
            ));
        }
        let path = package.join(SKILL_FILE);
        if !path.is_file() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "skill package does not contain SKILL.md",
            ));
        }
        return Ok(path);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "skill file was not found",
    ))
}

fn validate_skill_resource(resource: &ResourceId) -> Result<(), ToolError> {
    if resource.resource_type() != PROVIDER_ID || resource.tag() != "latest" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "skill resource must use type skill and the supported latest tag",
        ));
    }
    Ok(())
}

fn normalize_root(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            normalized.push(component.as_os_str());
        }
    }
    Some(normalized)
}
