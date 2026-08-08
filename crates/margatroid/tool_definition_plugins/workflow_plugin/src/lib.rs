use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use core_plugin::{App, Plugin};
use margatroid_types::{ResourceName, ResourceRef, ToolDefinition};
use serde_json::{json, Value};
use tool_plugin::{
    AgentToolEnvironment, AppToolExt, Tool, ToolDefinitionProvider, ToolError, ToolErrorKind,
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
        app.register_tool_provider(WorkflowToolProvider {
            home_root: self.home_root,
        });
    }
}

struct WorkflowToolProvider {
    home_root: Arc<PathBuf>,
}

impl ToolDefinitionProvider for WorkflowToolProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    fn provide(
        &self,
        environment: &AgentToolEnvironment,
        name: &ResourceName,
    ) -> Result<Tool, ToolError> {
        find_workflow_directory(environment, &self.home_root, name)?;
        let resource = ResourceRef::new(PROVIDER_ID, name.clone()).map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "workflow resource reference is invalid",
            )
        })?;
        let exposed_name = exposed_name(name)?;
        let result = format!(
            "Workflow `{}` is available, but workflow execution is not implemented yet.",
            name
        );
        Tool::new(
            resource,
            ToolDefinition {
                name: exposed_name,
                description: "Workflow execution is not implemented yet.".into(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": true
                }),
            },
            move |_context, _arguments: Value| {
                let result = result.clone();
                async move { Ok::<_, std::convert::Infallible>(result) }
            },
        )
    }
}

fn find_workflow_directory(
    environment: &AgentToolEnvironment,
    home_root: &Path,
    name: &ResourceName,
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
        let path = root.join(name.scope()).join(name.name());
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

fn exposed_name(name: &ResourceName) -> Result<String, ToolError> {
    let value = format!("workflow_{}_{}", name.scope(), name.name());
    if value.len() > 64 {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "workflow exposed name is too long",
        ));
    }
    Ok(value)
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use core_plugin::App;
    use margatroid_types::{AgentMessage, Message, ToolCall};
    use tempfile::tempdir;
    use tool_plugin::{ToolCallRequest, WorldToolExt};

    use super::*;

    #[test]
    fn existing_workflow_resolves_to_a_placeholder_tool() {
        let project = tempdir().unwrap();
        let image = tempdir().unwrap();
        let home = tempdir().unwrap();
        let workflow = project.path().join(".margatroid/workflows/local/review");
        std::fs::create_dir_all(&workflow).unwrap();

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(tool_plugin::ToolPlugin::default())
            .add_plugin(WorkflowPlugin::open(home.path()).unwrap());
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            AgentToolEnvironment::new(project.path(), image.path()),
        );
        let resource =
            ResourceRef::new("workflow", ResourceName::new("local/review").unwrap()).unwrap();
        let tool = app.world().resolve_tool(agent, &resource).unwrap();
        assert_eq!(tool.definition().name, "workflow_local_review");

        app.world().emit_event(ToolCallRequest {
            id: "turn-1".into(),
            agent,
            resource,
            call: ToolCall {
                id: "call-1".into(),
                name: tool.definition().name.clone(),
                arguments: "{}".into(),
            },
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(message) = app
                .world()
                .event_reader::<AgentMessage>()
                .into_iter()
                .next()
            {
                assert!(matches!(
                    message.message,
                    Message::Tool { ref content, .. }
                        if content.contains("execution is not implemented")
                ));
                break;
            }
            assert!(Instant::now() < deadline, "workflow execution timed out");
            std::thread::yield_now();
        }
    }
}
