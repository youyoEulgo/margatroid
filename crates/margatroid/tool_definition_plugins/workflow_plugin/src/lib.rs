use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use agent_plugin::Agent;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Entity, Event, Plugin, Resource, World};
use margatroid_types::ResourceId;
use serde_json::json;
use tool_plugin::{
    candidate_resource_entry, ResourceMapEntry, ToolCallRequest, ToolCallResponse, ToolError,
    ToolErrorKind, ToolTemplate,
};

const PROVIDER_ID: &str = "workflow";
const WORKFLOW_LOADER_ID: &str = "tool:builtin/workflow-loader:latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
}
impl Event for WorkflowRegisterRequest {}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
    pub result: Result<ResourceMapEntry, ToolError>,
}
impl Event for WorkflowRegisterResponse {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowErrorKind {
    InvalidRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowError {
    kind: WorkflowErrorKind,
    message: String,
}

impl WorkflowError {
    fn new(kind: WorkflowErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> WorkflowErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WorkflowError {}

pub struct WorkflowPlugin {
    home_root: Arc<PathBuf>,
}

struct WorkflowRoots {
    home_root: Arc<PathBuf>,
}
impl Resource for WorkflowRoots {}

impl WorkflowPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowError> {
        let home_root = normalize_root(home_root.into()).ok_or_else(|| {
            WorkflowError::new(
                WorkflowErrorKind::InvalidRoot,
                "workflow root must be absolute and cannot contain parent traversal",
            )
        })?;
        Ok(Self {
            home_root: Arc::new(home_root),
        })
    }
}

impl Plugin for WorkflowPlugin {
    fn build(self, app: &mut App) {
        app.world_mut().insert_resource(WorkflowRoots {
            home_root: self.home_root.clone(),
        });
        app.add_system(RuntimePlugin::UPDATE, workflow_register_system)
            .add_system(RuntimePlugin::UPDATE, workflow_tool_call_system);
    }
}

fn workflow_register_system(world: &mut World) {
    let requests = world
        .event_reader::<WorkflowRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = world
            .get_component::<Agent>(request.agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent tool environment is missing",
                )
            })
            .and_then(|agent| {
                validate_workflow_resource(&request.resource_id)?;
                let roots = world
                    .get_resource::<WorkflowRoots>()
                    .expect("WorkflowPlugin is installed");
                find_workflow_directory(
                    &agent.info.project_root,
                    &agent.info.image_root,
                    &roots.home_root,
                    &request.resource_id,
                )?;
                ToolTemplate::new(
                    request.resource_id.to_string(),
                    "Load this workflow resource.",
                    json!({"type":"object"}),
                )
            });
        let result = result.and_then(|template| {
            candidate_resource_entry(
                request.resource_id.clone(),
                request.alias.clone(),
                ResourceId::parse(WORKFLOW_LOADER_ID)
                    .expect("built-in Workflow loader ID is valid"),
                template,
            )
        });
        world.send_event(WorkflowRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn workflow_tool_call_system(world: &mut World) {
    let calls = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in calls
        .into_iter()
        .filter(|event| event.tool_id == ResourceId::parse(WORKFLOW_LOADER_ID).unwrap())
    {
        world.send_event(ToolCallResponse {
            turn_id: event.turn_id,
            agent: event.agent,
            tool_call_id: event.tool_call_id,
            result: Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "workflow execution is not implemented",
            )),
        });
    }
}

fn find_workflow_directory(
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
    resource: &ResourceId,
) -> Result<(), ToolError> {
    let candidates = [
        project_root.join(".margatroid").join("workflows"),
        image_root.join("workflows"),
        home_root.to_path_buf(),
    ];
    for root in candidates {
        let path = root
            .join(resource.scope())
            .join(resource.name())
            .join(resource.tag());
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "workflow package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "workflow package is not a directory",
            ));
        }
        return Ok(());
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "workflow resource was not found",
    ))
}

fn validate_workflow_resource(resource: &ResourceId) -> Result<(), ToolError> {
    if resource.resource_type() != PROVIDER_ID || resource.tag() != "latest" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "workflow resource must use type workflow and the supported latest tag",
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
