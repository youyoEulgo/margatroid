use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use margatroid_types::{
    AgentImageReference, ResourceName, ResourceRef, WorkspaceAgentDefinition, WorkspaceDefinition,
};
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
    resources: Vec<RawResource>,
    #[serde(default)]
    disable_resources: Vec<RawResource>,
    #[serde(default)]
    memory_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawResource {
    Structured(RawStructuredResource),
    Shorthand(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStructuredResource {
    provider: String,
    name: String,
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
        let image = AgentImageReference::new(raw_agent.image).map_err(|error| {
            ComposeError::new(
                ComposeErrorKind::InvalidImageReference,
                format!("invalid image reference: {error}"),
            )
        })?;
        let resources = raw_agent
            .resources
            .into_iter()
            .map(parse_resource)
            .collect::<Result<Vec<_>, _>>()?;
        let disable_resources = raw_agent
            .disable_resources
            .into_iter()
            .map(parse_resource)
            .collect::<Result<Vec<_>, _>>()?;
        let memory_path = raw_agent
            .memory_path
            .map(|path| resolve_path(&project_root, path))
            .transpose()?;
        agents.push(WorkspaceAgentDefinition {
            name,
            image,
            resources,
            disable_resources,
            memory_path,
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
    if agents.is_empty() {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidDefinition,
            "workspace must define at least one Agent",
        ));
    }
    let manager = raw.manager.unwrap_or_else(|| agents[0].name.clone());
    validate_logical_name(&manager, "manager")?;
    if !names.contains(manager.as_str()) {
        return Err(ComposeError::new(
            ComposeErrorKind::MissingManager,
            "workspace manager must name one configured Agent",
        ));
    }

    Ok(WorkspaceDefinition {
        name: raw.name,
        project_root,
        manager,
        agents,
    })
}

fn parse_resource(raw: RawResource) -> Result<ResourceRef, ComposeError> {
    let (provider, name) = match raw {
        RawResource::Structured(resource) => (resource.provider, resource.name),
        RawResource::Shorthand(value) => value.split_once(':').map_or_else(
            || {
                Err(ComposeError::new(
                    ComposeErrorKind::InvalidResourceReference,
                    "resource shorthand must use provider:scope/name",
                ))
            },
            |(provider, name)| Ok((provider.to_owned(), name.to_owned())),
        )?,
    };
    let name = ResourceName::new(name).map_err(|error| {
        ComposeError::new(
            ComposeErrorKind::InvalidResourceReference,
            format!("invalid resource reference: {error}"),
        )
    })?;
    ResourceRef::new(provider, name).map_err(|error| {
        ComposeError::new(
            ComposeErrorKind::InvalidResourceReference,
            format!("invalid resource reference: {error}"),
        )
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
    fn compiles_mapping_agents_and_resolves_paths() {
        let source = r#"
name: demo
project_root: project
manager: coder
agents:
  coder:
    image: local/coder
    resources:
      - provider: skill
        name: local/project-context
    memory_path: state/coder.sql
  reviewer:
    image: local/reviewer:stable
    disable_resources:
      - provider: workflow
        name: local/release
"#;
        let definition = compile_str(source, "/tmp/workspace/margatroid-workspace.yaml").unwrap();
        assert_eq!(definition.name, "demo");
        assert_eq!(
            definition.project_root,
            PathBuf::from("/tmp/workspace/project")
        );
        assert_eq!(definition.manager, "coder");
        assert_eq!(definition.agents[0].image.to_string(), "local/coder:latest");
        assert_eq!(definition.agents[1].name, "reviewer");
        assert_eq!(
            definition.agents[0].memory_path,
            Some(PathBuf::from("/tmp/workspace/project/state/coder.sql"))
        );
    }

    #[test]
    fn compiles_sequence_agents_and_shorthand_resources() {
        let source = r#"
name: demo
agents:
  - name: manager
    image: local/coder:v1
    resources: [skill:local/context]
"#;
        let definition = compile_str(source, "/tmp/workspace.yaml").unwrap();
        assert_eq!(definition.manager, "manager");
        assert_eq!(definition.agents[0].resources[0].provider(), "skill");
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
