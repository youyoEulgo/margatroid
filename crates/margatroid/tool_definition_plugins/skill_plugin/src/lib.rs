use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use core_plugin::{App, Plugin};
use margatroid_types::{ResourceName, ResourceRef, ToolDefinition};
use serde_json::{json, Value};
use tool_plugin::{
    AgentToolEnvironment, AppToolExt, Tool, ToolDefinitionProvider, ToolError, ToolErrorKind,
};

const PROVIDER_ID: &str = "skill";
const SKILL_FILE: &str = "SKILL.md";

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
        app.register_tool_provider(SkillToolProvider {
            home_root: self.home_root,
        });
    }
}

struct SkillToolProvider {
    home_root: Arc<PathBuf>,
}

impl ToolDefinitionProvider for SkillToolProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    fn provide(
        &self,
        environment: &AgentToolEnvironment,
        name: &ResourceName,
    ) -> Result<Tool, ToolError> {
        let path = find_skill_file(environment, &self.home_root, name)?;
        let content = fs::read_to_string(&path).map_err(|_| {
            ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "skill file could not be read",
            )
        })?;
        if content.trim().is_empty() {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "skill file is empty",
            ));
        }

        let resource = ResourceRef::new(PROVIDER_ID, name.clone()).map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "skill resource reference is invalid",
            )
        })?;
        let exposed_name = exposed_name(name)?;
        let description = content.clone();
        Tool::new(
            resource,
            ToolDefinition {
                name: exposed_name,
                description,
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": true
                }),
            },
            move |_context, _arguments: Value| {
                let content = content.clone();
                async move { Ok::<_, std::convert::Infallible>(content) }
            },
        )
    }
}

fn find_skill_file(
    environment: &AgentToolEnvironment,
    home_root: &Path,
    name: &ResourceName,
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
        let package = root.join(name.scope()).join(name.name());
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

fn exposed_name(name: &ResourceName) -> Result<String, ToolError> {
    let value = format!("skill_{}_{}", name.scope(), name.name());
    if value.len() > 64 {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill exposed name is too long",
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

    fn name(value: &str) -> ResourceName {
        ResourceName::new(value).unwrap()
    }

    #[test]
    fn project_skill_takes_priority_and_becomes_a_tool() {
        let project = tempdir().unwrap();
        let image = tempdir().unwrap();
        let home = tempdir().unwrap();
        let skill = project
            .path()
            .join(".margatroid/skills/local/review/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, "project skill").unwrap();

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(tool_plugin::ToolPlugin::default())
            .add_plugin(SkillPlugin::open(home.path()).unwrap());
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            AgentToolEnvironment::new(project.path(), image.path()),
        );
        let resource = ResourceRef::new("skill", name("local/review")).unwrap();
        let tool = app.world().resolve_tool(agent, &resource).unwrap();
        assert_eq!(tool.definition().name, "skill_local_review");
        assert_eq!(tool.definition().description, "project skill");

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
                assert_eq!(
                    message.message,
                    Message::Tool {
                        tool_call_id: "call-1".into(),
                        content: "project skill".into(),
                    }
                );
                break;
            }
            assert!(Instant::now() < deadline, "skill execution timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn invalid_project_package_does_not_fall_back_to_home() {
        let project = tempdir().unwrap();
        let image = tempdir().unwrap();
        let home = tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".margatroid/skills/local/review")).unwrap();
        let home_skill = home.path().join("local/review/SKILL.md");
        std::fs::create_dir_all(home_skill.parent().unwrap()).unwrap();
        std::fs::write(home_skill, "home skill").unwrap();

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(tool_plugin::ToolPlugin::default())
            .add_plugin(SkillPlugin::open(home.path()).unwrap());
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            AgentToolEnvironment::new(project.path(), image.path()),
        );
        let resource = ResourceRef::new("skill", name("local/review")).unwrap();

        let error = match app.world().resolve_tool(agent, &resource) {
            Ok(_) => panic!("invalid project skill must not fall back to home"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ToolErrorKind::ResourceResolutionFailed);
        assert_eq!(error.message(), "skill package does not contain SKILL.md");
    }
}
