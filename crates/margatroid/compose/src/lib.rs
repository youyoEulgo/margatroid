use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use margatroid_types::workspace_components::{WorkspaceAgentDefinition, WorkspaceDefinition};
use resource_id_plugin::ResourceID;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

mod storage;
pub use storage::FileWorkspaceCatalogStore;

const MAX_ERROR_MESSAGE_BYTES: usize = 512;
const MAX_LOGICAL_NAME_BYTES: usize = 128;
pub const MAX_WORKSPACE_YAML_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeErrorKind {
    FileRead,
    FileDecode,
    InvalidDefinition,
    InvalidPath,
    InvalidName,
    InvalidDriverReference,
    InvalidResourceReference,
    DuplicateAgent,
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
            let mut boundary = MAX_ERROR_MESSAGE_BYTES - 3;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for ComposeError {}

/// Compile a canonical `workspaces/<name>/workspace.yaml` file.
/// The ancestor of `workspaces` may be an injected temporary root.
pub fn compile(path: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError> {
    let path = path.as_ref();
    validate_workspace_file(path)?;
    let source = read_source(path)?;
    compile_str(&source, path)
}

pub(crate) fn open_regular(path: &Path) -> Result<fs::File, ComposeError> {
    reject_symlinks(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| ComposeError::new(ComposeErrorKind::FileRead, e.to_string()))?;
    if !file
        .metadata()
        .map_err(|e| ComposeError::new(ComposeErrorKind::FileRead, e.to_string()))?
        .is_file()
    {
        return Err(ComposeError::new(
            ComposeErrorKind::FileRead,
            "workspace source must be a regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn read_source(path: &Path) -> Result<String, ComposeError> {
    reject_symlinks(path)?;
    let file = open_regular(path)?;
    let mut source = String::new();
    file.take(MAX_WORKSPACE_YAML_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .map_err(|e| ComposeError::new(ComposeErrorKind::FileRead, e.to_string()))?;
    if source.len() > MAX_WORKSPACE_YAML_BYTES {
        return Err(ComposeError::new(
            ComposeErrorKind::FileRead,
            "workspace YAML exceeds size limit",
        ));
    }
    Ok(source)
}

/// Compile bounded YAML at its managed destination, which need not exist yet.
pub fn compile_str(
    source: &str,
    workspace_file: impl AsRef<Path>,
) -> Result<WorkspaceDefinition, ComposeError> {
    if source.len() > MAX_WORKSPACE_YAML_BYTES {
        return Err(ComposeError::new(
            ComposeErrorKind::FileDecode,
            "workspace YAML exceeds size limit",
        ));
    }
    let raw: WorkspaceFile = serde_yaml::from_str(source).map_err(|e| {
        ComposeError::new(
            ComposeErrorKind::FileDecode,
            format!("cannot decode workspace file: {e}"),
        )
    })?;
    let base = validate_workspace_file(workspace_file.as_ref())?;
    build_definition(raw, base)
}

fn validate_workspace_file(path: &Path) -> Result<&Path, ComposeError> {
    if !path.is_absolute() || normalize_path(path.to_path_buf())? != path {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "workspace file must be an absolute canonical path",
        ));
    }
    let base = path.parent().ok_or_else(|| {
        ComposeError::new(ComposeErrorKind::InvalidPath, "missing workspace directory")
    })?;
    if path.file_name().and_then(|v| v.to_str()) != Some("workspace.yaml")
        || base
            .parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            != Some("workspaces")
    {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "expected workspaces/<name>/workspace.yaml",
        ));
    }
    reject_symlinks(path)?;
    Ok(base)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFile {
    name: String,
    project_root: PathBuf,
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
    driver: String,
    #[serde(default)]
    home_root: Option<PathBuf>,
}

fn build_definition(raw: WorkspaceFile, base: &Path) -> Result<WorkspaceDefinition, ComposeError> {
    validate_logical_name(&raw.name, "workspace")?;
    if base.file_name().and_then(|v| v.to_str()) != Some(&raw.name) {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidName,
            "workspace name must match directory",
        ));
    }
    let project_root = normalize_path(raw.project_root)?;
    let canonical = fs::canonicalize(&project_root).map_err(|e| {
        ComposeError::new(
            ComposeErrorKind::InvalidPath,
            format!("project_root must exist: {e}"),
        )
    })?;
    if canonical != project_root || !project_root.is_dir() {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "project_root must be an existing canonical directory",
        ));
    }
    let mut agents = Vec::with_capacity(raw.agents.0.len());
    let mut names = std::collections::HashSet::new();
    for agent in raw.agents.0 {
        let name = agent.name.ok_or_else(|| {
            ComposeError::new(
                ComposeErrorKind::InvalidDefinition,
                "sequence agents require name",
            )
        })?;
        validate_logical_name(&name, "agent")?;
        if !names.insert(name.clone()) {
            return Err(ComposeError::new(
                ComposeErrorKind::DuplicateAgent,
                "duplicate agent name",
            ));
        }
        let value = if agent.driver.starts_with("driver:") {
            agent.driver
        } else {
            format!("driver:{}", agent.driver)
        };
        let driver = ResourceID::parse(value).map_err(|e| {
            ComposeError::new(ComposeErrorKind::InvalidDriverReference, e.to_string())
        })?;
        if driver.resource_type() != "driver" {
            return Err(ComposeError::new(
                ComposeErrorKind::InvalidDriverReference,
                "expected driver address",
            ));
        }
        let home_root = match agent.home_root {
            Some(path) if path.is_absolute() => normalize_path(path)?,
            Some(path) => normalize_path(project_root.join(path))?,
            None => normalize_path(PathBuf::from(std::env::var_os("HOME").ok_or_else(
                || ComposeError::new(ComposeErrorKind::InvalidPath, "HOME is not configured"),
            )?))?,
        };
        let history_path = base.join("agents").join(&name).join("state.sqlite");
        reject_symlinks(&history_path)?;
        agents.push(WorkspaceAgentDefinition {
            id: ResourceID::new("agent", &raw.name, &name, None::<String>).map_err(|e| {
                ComposeError::new(ComposeErrorKind::InvalidDefinition, e.to_string())
            })?,
            name,
            driver,
            home_root,
            history_path,
        });
    }
    Ok(WorkspaceDefinition {
        id: ResourceID::new("workspace", "local", &raw.name, None::<String>)
            .map_err(|e| ComposeError::new(ComposeErrorKind::InvalidDefinition, e.to_string()))?,
        name: raw.name,
        project_root,
        agents,
    })
}

pub(crate) fn validate_logical_name(value: &str, kind: &str) -> Result<(), ComposeError> {
    if value.is_empty()
        || value.len() > MAX_LOGICAL_NAME_BYTES
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidName,
            format!("{kind} name is invalid"),
        ));
    }
    Ok(())
}

pub(crate) fn normalize_path(path: PathBuf) -> Result<PathBuf, ComposeError> {
    if !path.is_absolute() {
        return Err(ComposeError::new(
            ComposeErrorKind::InvalidPath,
            "path must be absolute",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir | Component::CurDir => {
                return Err(ComposeError::new(
                    ComposeErrorKind::InvalidPath,
                    "path cannot contain traversal or dot components",
                ))
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    Ok(normalized)
}

pub(crate) fn reject_symlinks(path: &Path) -> Result<(), ComposeError> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ComposeError::new(
                    ComposeErrorKind::InvalidPath,
                    "managed data paths cannot contain symlinks",
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ComposeError::new(
                    ComposeErrorKind::InvalidPath,
                    e.to_string(),
                ))
            }
        }
    }
    Ok(())
}
