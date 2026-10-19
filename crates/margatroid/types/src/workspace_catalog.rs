//! Persistent workspace definitions. Calls block; run them outside the ECS world lock.
use crate::workspace_components::{WorkspaceDefinition, WorkspaceError};
use core_plugin::Resource;
use resource_id_plugin::ResourceID;
use std::{path::PathBuf, sync::Arc};

/// Canonical persistent address of a local workspace name.
pub fn workspace_resource_id(name: &str) -> Result<ResourceID, WorkspaceError> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(WorkspaceError::new(
            crate::workspace_components::WorkspaceErrorKind::InvalidDefinition,
            "workspace name is invalid",
        ));
    }
    ResourceID::new("workspace", "local", name, None).map_err(|error| {
        WorkspaceError::new(
            crate::workspace_components::WorkspaceErrorKind::InvalidDefinition,
            error.to_string(),
        )
    })
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceCatalogEntry {
    pub id: ResourceID,
    pub name: String,
    pub path: PathBuf,
    /// Exact source-content hash; empty when no bounded source can be read.
    pub revision: String,
    pub definition: Option<WorkspaceDefinition>,
    pub error: Option<String>,
}

/// Result of a recoverable removal: the workspace directory was moved intact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDeletion {
    /// Workspace address removed from the catalog.
    pub id: ResourceID,
    /// Original `workspaces/<name>` directory, now absent.
    pub path: PathBuf,
    /// New location of the intact directory, outside the catalog listing.
    pub removed_path: PathBuf,
}

pub trait WorkspaceCatalogStore: Send + Sync + 'static {
    fn list(&self) -> Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>;
    fn load(
        &self,
        id: &ResourceID,
        expected_revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError>;
    /// Empty expected revision creates only; other revisions compare exact source content.
    /// Lifecycle must hold its operation lease across the active-state check and this call.
    fn save(
        &self,
        definition: &WorkspaceDefinition,
        expected_revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError>;
    /// Recoverably rename a stopped workspace and migrate its persistent member identities.
    /// Rejected when the new name already exists or the old name is unchanged.
    /// `definition` must be the current SOURCE definition compiled at `expected_revision`,
    /// including the old workspace/member IDs and paths. The store derives the destination.
    /// The lifecycle lease covers handle closure and the entire migration; failures after
    /// durable commit can require recovery rather than implying an atomic rollback.
    fn rename(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        new_name: &str,
        definition: &WorkspaceDefinition,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        let _ = (id, expected_revision, new_name, definition);
        Err(WorkspaceError::new(
            crate::workspace_components::WorkspaceErrorKind::InvalidDefinition,
            "workspace rename unsupported",
        ))
    }
    /// Rename using the current SOURCE definition, with old IDs and paths, as in `rename`.
    fn rename_workspace(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        new_name: &str,
        definition: &WorkspaceDefinition,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        self.rename(id, expected_revision, new_name, definition)
    }
    fn rename_member(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        agent: &ResourceID,
        new_name: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        let _ = (id, expected_revision, agent, new_name);
        Err(WorkspaceError::new(
            crate::workspace_components::WorkspaceErrorKind::InvalidDefinition,
            "member rename unsupported",
        ))
    }
    /// Atomically move the whole `workspaces/<name>` directory out of the catalog root. The
    /// implementation chooses the recoverable destination; nothing is destroyed.
    fn delete(
        &self,
        id: &ResourceID,
        expected_revision: &str,
    ) -> Result<WorkspaceDeletion, WorkspaceError> {
        let _ = (id, expected_revision);
        Err(WorkspaceError::new(
            crate::workspace_components::WorkspaceErrorKind::InvalidDefinition,
            "workspace delete unsupported",
        ))
    }
    fn delete_workspace(
        &self,
        id: &ResourceID,
        expected_revision: &str,
    ) -> Result<WorkspaceDeletion, WorkspaceError> {
        self.delete(id, expected_revision)
    }
}

#[derive(Clone)]
pub struct WorkspaceCatalogHandle(pub Arc<dyn WorkspaceCatalogStore>);
impl Resource for WorkspaceCatalogHandle {}

/// Snapshot refreshed by lifecycle after storage operations; never starts workspaces.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceCatalog(pub Vec<WorkspaceCatalogEntry>);
impl Resource for WorkspaceCatalog {}

/// Driver addresses discovered from the configured AgentDriver root.
///
/// Published by the driver loader (or the composition root through its scan helper) as a
/// plain `Resource`; the workspace lifecycle only validates membership when this is installed
/// so catalogs without driver scanning keep working.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriverCatalog(pub Vec<ResourceID>);
impl Resource for DriverCatalog {}
impl DriverCatalog {
    pub fn contains(&self, id: &ResourceID) -> bool {
        self.0.iter().any(|candidate| candidate == id)
    }
}
