pub mod components;
pub mod error;
pub mod events;
pub mod types;

pub use components::{Workspace, WorkspaceExecutor};
pub use error::{WorkspaceError, WorkspaceErrorKind};
pub use events::*;
pub use types::*;

use core_plugin::{Entity, Resource, World};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Clone, Debug, Default)]
pub struct WorkspaceDefinitions {
    pub entries: Arc<RwLock<BTreeMap<PathBuf, WorkspaceDefinition>>>,
}
impl Resource for WorkspaceDefinitions {}
impl WorkspaceDefinitions {
    pub fn get(&self, path: &Path) -> Option<WorkspaceDefinition> {
        self.entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(path)
            .cloned()
    }
    pub fn list(&self) -> Vec<(PathBuf, WorkspaceDefinition)> {
        self.entries
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .map(|(path, definition)| (path.clone(), definition.clone()))
            .collect()
    }
}
pub trait WorldWorkspaceExt {
    fn workspace_definitions(&self) -> Result<&WorkspaceDefinitions, WorkspaceError>;
    fn workspace_state(&self, workspace: Entity) -> Result<&WorkspaceState, WorkspaceError>;
    fn workspace_agents(&self, workspace: Entity) -> Result<Vec<Entity>, WorkspaceError>;
    fn workspace_insert_agent(
        &mut self,
        workspace: Entity,
        agent: Entity,
    ) -> Result<(), WorkspaceError>;
    fn workspace_remove_agent(
        &mut self,
        workspace: Entity,
        agent: Entity,
    ) -> Result<(), WorkspaceError>;
}
fn missing() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::Missing,
        "workspace entity or component is missing",
    )
}
impl WorldWorkspaceExt for World {
    fn workspace_definitions(&self) -> Result<&WorkspaceDefinitions, WorkspaceError> {
        self.get_resource::<WorkspaceDefinitions>().ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::Missing,
                "WorkspaceDefinitions is not installed",
            )
        })
    }
    fn workspace_state(&self, workspace: Entity) -> Result<&WorkspaceState, WorkspaceError> {
        Ok(&self
            .get_component::<Workspace>(workspace)
            .ok_or_else(missing)?
            .state)
    }
    fn workspace_agents(&self, workspace: Entity) -> Result<Vec<Entity>, WorkspaceError> {
        Ok(self
            .get_component::<Workspace>(workspace)
            .ok_or_else(missing)?
            .agents
            .iter()
            .copied()
            .collect())
    }
    fn workspace_insert_agent(
        &mut self,
        workspace: Entity,
        agent: Entity,
    ) -> Result<(), WorkspaceError> {
        if !self.is_alive(agent) {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::Missing,
                "member entity is not alive",
            ));
        }
        self.get_component_mut::<Workspace>(workspace)
            .ok_or_else(missing)?
            .agents
            .insert(agent);
        Ok(())
    }
    fn workspace_remove_agent(
        &mut self,
        workspace: Entity,
        agent: Entity,
    ) -> Result<(), WorkspaceError> {
        self.get_component_mut::<Workspace>(workspace)
            .ok_or_else(missing)?
            .agents
            .remove(&agent);
        Ok(())
    }
}

pub const MAX_WORKSPACE_DEFINITION_BYTES: usize = 1024 * 1024;
pub const MAX_WORKSPACE_MEMBERS: usize = 1024;
pub fn load_workspace_definition(path: &Path) -> Result<WorkspaceDefinition, WorkspaceError> {
    let invalid = |message: String| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidDefinition,
            format!("{}: {message}", path.display()),
        )
    };
    let resolved = std::fs::canonicalize(path).map_err(|e| invalid(e.to_string()))?;
    let metadata = std::fs::metadata(&resolved).map_err(|e| invalid(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_WORKSPACE_DEFINITION_BYTES as u64 {
        return Err(invalid(
            "workspace definition must be a regular YAML file of at most 1 MiB".into(),
        ));
    }
    let file = File::open(&resolved).map_err(|e| invalid(e.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_WORKSPACE_DEFINITION_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| invalid(e.to_string()))?;
    if bytes.len() > MAX_WORKSPACE_DEFINITION_BYTES {
        return Err(invalid("workspace definition exceeds 1 MiB".into()));
    }
    let document: WorkspaceDocument =
        serde_yaml::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    let base = resolved
        .parent()
        .ok_or_else(|| invalid("definition path has no parent".into()))?;
    let definition = normalize_document(document, base)?;
    validate_workspace_definition(&definition)?;
    Ok(definition)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDocument {
    name: String,
    project_root: PathBuf,
    #[serde(default)]
    members: DocumentField<MemberList>,
    #[serde(default)]
    agents: DocumentField<AgentMap>,
}

#[derive(Default)]
enum DocumentField<T> {
    #[default]
    Missing,
    Present(T),
}
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for DocumentField<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberDocument {
    name: Option<String>,
    driver: String,
    home_root: Option<PathBuf>,
}
struct MemberList(Vec<MemberDocument>);
impl<'de> serde::Deserialize<'de> for MemberList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MembersVisitor;
        impl<'de> serde::de::Visitor<'de> for MembersVisitor {
            type Value = MemberList;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sequence of workspace members")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = sequence.next_element::<MemberDocument>()? {
                    if members.len() >= MAX_WORKSPACE_MEMBERS {
                        return Err(serde::de::Error::custom(
                            "workspace has more than 1024 members",
                        ));
                    }
                    members.push(member);
                }
                Ok(MemberList(members))
            }
        }
        deserializer.deserialize_seq(MembersVisitor)
    }
}
struct AgentMap(Vec<MemberDocument>);
impl<'de> serde::Deserialize<'de> for AgentMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AgentsVisitor;
        impl<'de> serde::de::Visitor<'de> for AgentsVisitor {
            type Value = AgentMap;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map from member names to workspace agents")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut members = Vec::new();
                let mut names = BTreeSet::new();
                while let Some((name, mut member)) = map.next_entry::<String, MemberDocument>()? {
                    if members.len() >= MAX_WORKSPACE_MEMBERS {
                        return Err(serde::de::Error::custom(
                            "workspace has more than 1024 members",
                        ));
                    }
                    if !names.insert(name.clone()) {
                        return Err(serde::de::Error::custom("duplicate workspace agent name"));
                    }
                    if member.name.as_deref().is_some_and(|inner| inner != name) {
                        return Err(serde::de::Error::custom(
                            "agent name must match its mapping key",
                        ));
                    }
                    member.name = Some(name);
                    members.push(member);
                }
                Ok(AgentMap(members))
            }
        }
        deserializer.deserialize_map(AgentsVisitor)
    }
}
fn invalid_definition(message: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(WorkspaceErrorKind::InvalidDefinition, message)
}
fn normalize_path(path: PathBuf) -> Result<PathBuf, WorkspaceError> {
    if !path.is_absolute() {
        return Err(invalid_definition("workspace paths must be absolute"));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(invalid_definition(
                        "workspace path traverses above its root",
                    ));
                }
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    Ok(normalized)
}
fn normalize_driver(driver: String) -> Result<String, WorkspaceError> {
    let normalized = if driver.starts_with("driver:") {
        driver
    } else {
        format!("driver:{driver}")
    };
    let invalid = || invalid_definition("driver must be a driver:scope/name[:tag] file locator");
    let body = normalized.strip_prefix("driver:").ok_or_else(invalid)?;
    let (scope, name_and_tag) = body.split_once('/').ok_or_else(invalid)?;
    let (name, tag) = match name_and_tag.split_once(':') {
        Some((name, tag)) => (name, Some(tag)),
        None => (name_and_tag, None),
    };
    let valid_part = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && !part.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || matches!(character, '/' | '\\' | ':')
            })
    };
    if !valid_part(scope) || !valid_part(name) || tag.is_some_and(|tag| !valid_part(tag)) {
        return Err(invalid());
    }
    Ok(normalized)
}
fn normalize_document(
    document: WorkspaceDocument,
    base: &Path,
) -> Result<WorkspaceDefinition, WorkspaceError> {
    let (members, legacy) = match (document.members, document.agents) {
        (DocumentField::Missing, DocumentField::Missing) => (Vec::new(), false),
        (DocumentField::Present(MemberList(members)), DocumentField::Missing) => (members, false),
        (DocumentField::Missing, DocumentField::Present(AgentMap(members))) => (members, true),
        _ => {
            return Err(invalid_definition(
                "workspace agents and members are mutually exclusive",
            ))
        }
    };
    if document.project_root.as_os_str().is_empty() {
        return Err(invalid_definition("project_root must not be empty"));
    }
    let project_root = normalize_path(if document.project_root.is_absolute() {
        document.project_root
    } else {
        base.join(document.project_root)
    })?;
    let mut normalized = Vec::with_capacity(members.len());
    for member in members {
        let name = member
            .name
            .ok_or_else(|| invalid_definition("members entries require a name"))?;
        let home_root = match member.home_root {
            Some(path) if path.as_os_str().is_empty() => {
                return Err(invalid_definition("home_root must not be empty"))
            }
            Some(path) if path.is_absolute() => normalize_path(path)?,
            Some(path) => normalize_path(if legacy {
                project_root.join(path)
            } else {
                base.join(path)
            })?,
            None => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|home| home.is_absolute())
                    .ok_or_else(|| {
                        invalid_definition(
                            "HOME must name an absolute directory when home_root is omitted",
                        )
                    })?;
                normalize_path(home)?
            }
        };
        normalized.push(WorkspaceMemberDefinition {
            name,
            driver: normalize_driver(member.driver)?,
            home_root,
        });
    }
    Ok(WorkspaceDefinition {
        name: document.name,
        project_root,
        members: normalized,
    })
}
pub fn validate_workspace_definition(
    definition: &WorkspaceDefinition,
) -> Result<(), WorkspaceError> {
    let invalid =
        |message: &str| WorkspaceError::new(WorkspaceErrorKind::InvalidDefinition, message);
    let valid_text = |value: &str| !value.trim().is_empty() && !value.chars().any(char::is_control);
    if !valid_text(&definition.name) || definition.project_root.as_os_str().is_empty() {
        return Err(invalid("workspace name and project_root must not be empty"));
    }
    if definition.members.len() > MAX_WORKSPACE_MEMBERS {
        return Err(invalid("workspace has more than 1024 members"));
    }
    let mut names = BTreeSet::new();
    for member in &definition.members {
        if !valid_text(&member.name)
            || !valid_text(&member.driver)
            || member.home_root.as_os_str().is_empty()
        {
            return Err(invalid(
                "member name, driver and home_root must not be empty or contain control characters",
            ));
        }
        if !names.insert(&member.name) {
            return Err(invalid("workspace member names must be unique"));
        }
    }
    Ok(())
}
