use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Plugin, Resource, World};
use margatroid_types::{ResourceId, ToolDefinition};
use serde_json::json;
use tool_plugin::{
    AgentToolEnvironment, AppToolExt, ToolCallEvent, ToolDefinitionResult, ToolDefinitionRoute,
    ToolError, ToolErrorKind, ToolTemplate,
};

const PROVIDER_ID: &str = "workflow";

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
        app.register_tool_template(
            ToolTemplate::new(
                ResourceId::parse("tool:builtin/workflow-loader:latest").unwrap(),
                ToolDefinition {
                    name: "workflow_loader".into(),
                    description: "Load a workflow resource by its complete resource ID.".into(),
                    input_schema: json!({"type":"object"}),
                },
            )
            .unwrap(),
        );
        app.add_system(RuntimePlugin::UPDATE, workflow_tool_definition_system)
            .add_system(RuntimePlugin::UPDATE, workflow_tool_call_system);
    }
}

fn workflow_tool_definition_system(world: &mut World) {
    let routes = world
        .event_reader::<ToolDefinitionRoute>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for route in routes.into_iter().filter(|route| {
        route.loader == ResourceId::parse("tool:builtin/workflow-loader:latest").unwrap()
    }) {
        let result = world
            .get_component::<AgentToolEnvironment>(route.agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent tool environment is missing",
                )
            })
            .and_then(|environment| {
                validate_workflow_resource(&route.resource)?;
                let roots = world
                    .get_resource::<WorkflowRoots>()
                    .expect("WorkflowPlugin is installed");
                find_workflow_directory(environment, &roots.home_root, &route.resource)?;
                Ok(ToolDefinition {
                    name: "workflow_loader".into(),
                    description: "Load a workflow resource.".into(),
                    input_schema: json!({"type":"object"}),
                })
            });
        world.send_event(ToolDefinitionResult {
            id: route.id,
            agent: route.agent,
            resource: route.resource,
            result,
        });
    }
}

fn workflow_tool_call_system(world: &mut World) {
    let calls = world
        .event_reader::<ToolCallEvent>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in calls.into_iter().filter(|event| {
        event.loader == ResourceId::parse("tool:builtin/workflow-loader:latest").unwrap()
    }) {
        let content = "Workflow execution is not implemented yet.".to_owned();
        world.send_event(margatroid_types::AgentMessage {
            id: event.id,
            agent: event.agent,
            message: margatroid_types::Message::Tool {
                tool_call_id: event.call.id,
                content,
            },
        });
    }
}

fn find_workflow_directory(
    environment: &AgentToolEnvironment,
    home_root: &Path,
    resource: &ResourceId,
) -> Result<(), ToolError> {
    let candidates = [
        environment
            .project_root()
            .join(".margatroid")
            .join("workflows"),
        environment.image_root().join("workflows"),
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
