use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use margatroid_types::workspace_components::{WorkspaceAgentDefinition, WorkspaceDefinition};
use resource_id_plugin::ResourceID;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

const MAX_ERROR_MESSAGE_BYTES: usize = 512;
const MAX_LOGICAL_NAME_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeErrorKind {
    FileRead,
    FileDecode,
    InvalidDefinition,
    InvalidPath,
    InvalidName,
    InvalidImageReference,
    InvalidResourceReference,
    DuplicateAgent,
    MissingManager,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeError {
    kind: ComposeErrorKind,
    message: String,
}

impl ComposeError {
    fn new(kind: ComposeErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_MESSAGE_BYTES {
            let suffix = "...";
            let mut boundary = MAX_ERROR_MESSAGE_BYTES - suffix.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(suffix);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> ComposeErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ComposeError {}

/// Compile a `margatroid-workspace.yaml` file into a runtime-independent definition.
pub fn compile(path: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError> {
    let input_path = path.as_ref();
    let file_path = fs::canonicalize(input_path).map_err(|error| {
        ComposeError::new(
            ComposeErrorKind::FileRead,
            format!("cannot read workspace file: {error}"),
        )
    })?;
    let source = fs::read_to_string(&file_path).map_err(|error| {
        ComposeError::new(
            ComposeErrorKind::FileRead,
            format!("cannot read workspace file: {error}"),
        )
    })?;
    compile_str(&source, file_path)
}

/// Compile YAML text using `workspace_file` as the base for relative paths.
pub fn compile_str(
    source: &str,
    workspace_file: impl AsRef<Path>,
) -> Result<WorkspaceDefinition, ComposeError> {
    let raw: WorkspaceFile = serde_yaml::from_str(source).map_err(|error| {
        ComposeError::new(
            ComposeErrorKind::FileDecode,
            format!("cannot decode workspace file: {error}"),
        )
    })?;
    let workspace_file = absolute_path(workspace_file.as_ref())?;
    let base = workspace_file.parent().ok_or_else(|| {
        ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "workspace file has no parent directory",
        )
    })?;
    build_definition(raw, base)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFile {
    name: String,
    #[serde(default)]
    project_root: Option<PathBuf>,
    #[serde(default)]
    manager: Option<String>,
    agents: AgentDocuments,
}

#[derive(Debug, Default)]
struct AgentDocuments(Vec<RawAgent>);

impl<'de> Deserialize<'de> for AgentDocuments {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AgentDocumentsVisitor;

        impl<'de> Visitor<'de> for AgentDocumentsVisitor {
            type Value = AgentDocuments;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map or sequence of agent definitions")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut agents = Vec::new();
                while let Some((name, mut agent)) = map.next_entry::<String, RawAgent>()? {
                    if let Some(inner_name) = agent.name.as_deref() {
                        if inner_name != name {
                            return Err(de::Error::custom("agent name must match its mapping key"));
                        }
                    }
                    agent.name = Some(name);
                    agents.push(agent);
                }
                Ok(AgentDocuments(agents))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut agents = Vec::new();
                while let Some(agent) = sequence.next_element::<RawAgent>()? {
                    agents.push(agent);
                }
                Ok(AgentDocuments(agents))
            }
        }

        deserializer.deserialize_any(AgentDocumentsVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgent {
    #[serde(default)]
    name: Option<String>,
    image: String,
    #[serde(default)]
    home_root: Option<PathBuf>,
    #[serde(default, alias = "memory_path")]
    history_path: Option<PathBuf>,
}

fn build_definition(raw: WorkspaceFile, base: &Path) -> Result<WorkspaceDefinition, ComposeError> {
    validate_logical_name(&raw.name, "workspace")?;
    let project_root = raw
        .project_root
        .map(|path| resolve_path(base, path))
        .transpose()?
        .unwrap_or_else(|| base.to_path_buf());

    let mut agents = Vec::with_capacity(raw.agents.0.len());
    for raw_agent in raw.agents.0 {
        let name = raw_agent.name.ok_or_else(|| {
            ComposeError::new(
                ComposeErrorKind::InvalidDefinition,
                "sequence agent definitions must contain name",
            )
        })?;
        validate_logical_name(&name, "agent")?;
        let image_value = if raw_agent.image.starts_with("image:") {
            raw_agent.image
        } else {
            format!("image:{}", raw_agent.image)
        };
        let image = ResourceID::parse(image_value).map_err(|error| {
            ComposeError::new(
                ComposeErrorKind::InvalidImageReference,
                format!("invalid image reference: {error}"),
            )
        })?;
        if image.resource_type() != "image" {
            return Err(ComposeError::new(
                ComposeErrorKind::InvalidImageReference,
                "image reference must use type image",
            ));
        }
        let home_root = match raw_agent.home_root {
            Some(path) => resolve_path(&project_root, path)?,
            None => normalize_path(PathBuf::from(std::env::var_os("HOME").ok_or_else(
                || ComposeError::new(ComposeErrorKind::InvalidPath, "HOME is not configured"),
            )?))?,
        };
        let history_path = raw_agent
            .history_path
            .map(|path| resolve_path(&project_root, path))
            .transpose()?
            .unwrap_or_else(|| {
                project_root
                    .join(".margatroid/history")
                    .join(format!("{name}.sqlite"))
            });
        agents.push(WorkspaceAgentDefinition {
            id: ResourceID::new("agent", &raw.name, &name, None::<String>).map_err(|error| {
                ComposeError::new(
                    ComposeErrorKind::InvalidDefinition,
                    format!("invalid Agent resource ID: {error}"),
                )
            })?,
            name,
            image,
            home_root,
            history_path,
        });
    }

    let mut names = std::collections::HashSet::with_capacity(agents.len());
    for agent in &agents {
        if !names.insert(agent.name.as_str()) {
            return Err(ComposeError::new(
                ComposeErrorKind::DuplicateAgent,
                "workspace Agent names must be unique",
            ));
        }
    }
    let manager = raw.manager.unwrap_or_else(|| {
        agents
            .first()
            .map(|agent| agent.name.clone())
            .unwrap_or_default()
    });
    if !agents.is_empty() {
        validate_logical_name(&manager, "manager")?;
    }
    if (agents.is_empty() && !manager.is_empty())
        || (!agents.is_empty() && !names.contains(manager.as_str()))
    {
        return Err(ComposeError::new(
            ComposeErrorKind::MissingManager,
            "workspace manager must name one configured Agent",
        ));
    }

    Ok(WorkspaceDefinition {
        id: ResourceID::new("workspace", "local", &raw.name, None::<String>).map_err(|error| {
            ComposeError::new(ComposeErrorKind::InvalidDefinition, error.to_string())
        })?,
        name: raw.name,
        project_root,
        manager,
        agents,
    })
}

fn validate_logical_name(value: &str, kind: &str) -> Result<(), ComposeError> {
    if value.is_empty()
        || value.len() > MAX_LOGICAL_NAME_BYTES
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidName,
            format!("{kind} name is invalid"),
        ));
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, ComposeError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let current = std::env::current_dir().map_err(|error| {
            ComposeError::new(
                ComposeErrorKind::InvalidPath,
                format!("cannot determine current directory: {error}"),
            )
        })?;
        current.join(path)
    };
    normalize_path(candidate)
}

fn resolve_path(base: &Path, path: PathBuf) -> Result<PathBuf, ComposeError> {
    let candidate = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    normalize_path(candidate)
}

fn normalize_path(path: PathBuf) -> Result<PathBuf, ComposeError> {
    if !path.is_absolute() {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "resolved path must be absolute",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ComposeError::new(
                    ComposeErrorKind::InvalidPath,
                    "path cannot contain parent traversal",
                ));
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_empty_workspace_with_empty_manager() {
        for source in [
            "name: empty\nagents:	[]\n",
            "name: empty\nmanager: ''\nagents:	{}\n",
        ] {
            let definition = compile_str(source, "/tmp/workspace.yaml").unwrap();
            assert!(definition.agents.is_empty());
            assert!(definition.manager.is_empty());
            assert_eq!(definition.id.to_string(), "workspace:local/empty:latest");
        }
        assert_eq!(
            compile_str(
                "name: empty\nmanager: missing\nagents:	[]\n",
                "/tmp/workspace.yaml"
            )
            .unwrap_err()
            .kind(),
            ComposeErrorKind::MissingManager
        );
    }

    #[test]
    fn rejects_legacy_resource_lists_and_agent_tag() {
        for field in [
            "resources",
            "disable_resources",
            "tool_default",
            "tool_dynamic",
        ] {
            let source =
                format!("name: demo\nagents:\n  coder:\n    image: local/coder\n    {field}:	[]\n");
            assert_eq!(
                compile_str(&source, "/tmp/workspace.yaml")
                    .unwrap_err()
                    .kind(),
                ComposeErrorKind::FileDecode,
                "{field}"
            );
        }
        assert_eq!(
            compile_str(
                "name: demo\nagents:\n  coder:\n    image: local/coder\n    tag: clone0\n",
                "/tmp/workspace.yaml"
            )
            .unwrap_err()
            .kind(),
            ComposeErrorKind::FileDecode
        );
    }

    #[test]
    fn compiles_mapping_agents_and_resolves_paths() {
        let source = r#"
name: demo
project_root: project
manager: coder
agents:
  coder:
    image: local/coder
    memory_path: state/coder.sql
  reviewer:
    image: local/reviewer:stable
"#;
        let definition = compile_str(source, "/tmp/workspace/margatroid-workspace.yaml").unwrap();
        assert_eq!(definition.name, "demo");
        assert_eq!(
            definition.project_root,
            PathBuf::from("/tmp/workspace/project")
        );
        assert_eq!(definition.manager, "coder");
        assert_eq!(
            definition.agents[0].id.to_string(),
            "agent:demo/coder:latest"
        );
        assert_eq!(
            definition.agents[0].image.to_string(),
            "image:local/coder:latest"
        );
        assert_eq!(definition.agents[1].name, "reviewer");
        assert_eq!(
            definition.agents[0].history_path,
            PathBuf::from("/tmp/workspace/project/state/coder.sql")
        );
    }

    #[test]
    fn compiles_sequence_agents_and_history_paths() {
        let source = r#"
name: demo
agents:
  - name: manager
    image: local/coder:v1
"#;
        let definition = compile_str(source, "/tmp/workspace.yaml").unwrap();
        assert_eq!(definition.manager, "manager");
        assert!(definition.agents[0]
            .history_path
            .ends_with("manager.sqlite"));
    }

    #[test]
    fn rejects_invalid_manager_and_parent_paths() {
        let source = r#"
name: demo
project_root: ../project
manager: missing
agents:
  manager:
    image: local/coder
"#;
        let error = compile_str(source, "/tmp/workspace.yaml").unwrap_err();
        assert_eq!(error.kind(), ComposeErrorKind::InvalidPath);
    }

    #[test]
    fn compile_reads_a_workspace_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("margatroid-workspace.yaml");
        fs::write(
            &path,
            "name: demo\nagents:\n  manager:\n    image: local/coder\n",
        )
        .unwrap();
        assert_eq!(compile(&path).unwrap().project_root, directory.path());
    }
}
