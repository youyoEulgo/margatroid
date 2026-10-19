use crate::workspace_catalog::{WorkspaceCatalogEntry, WorkspaceDeletion};
use crate::workspace_components::{WorkspaceDefinition, WorkspaceError};
use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct StartWorkspace {
    pub id: String,
    pub definition: WorkspaceDefinition,
}
impl Event for StartWorkspace {}
#[derive(Clone, Debug)]
pub struct StartWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for StartWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
}
impl Event for StopWorkspace {}
#[derive(Clone, Debug)]
pub struct StopWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for StopWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct WorkspaceExecutionCloseOutput {
    pub workspace: Entity,
    pub request_id: String,
    pub result: Result<(), WorkspaceError>,
}
impl Event for WorkspaceExecutionCloseOutput {}
#[derive(Clone, Debug)]
pub struct WorkspaceException {
    pub exception_id: String,
    pub workspace: Entity,
    pub resource_id: ResourceID,
    pub agent: Entity,
    pub agent_resource_id: ResourceID,
    pub error: WorkspaceError,
}
impl Event for WorkspaceException {}
#[derive(Clone, Debug)]
pub struct WorkspaceExceptionResult {
    pub exception_id: String,
    pub workspace: Entity,
    pub resource_id: ResourceID,
    pub agent: Entity,
    pub agent_resource_id: ResourceID,
    pub error: Option<WorkspaceError>,
    pub result: Result<(), WorkspaceError>,
}
impl Event for WorkspaceExceptionResult {}

#[derive(Clone, Debug)]
pub struct WorkspaceMemberStartFailed {
    pub workspace: Entity,
    pub workspace_id: ResourceID,
    pub agent: ResourceID,
    pub driver: ResourceID,
    pub error: WorkspaceError,
}
impl Event for WorkspaceMemberStartFailed {}

#[derive(Clone, Debug)]
pub struct WorkspaceMemberSpec {
    pub name: String,
    pub driver: ResourceID,
    pub home_root: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct RefreshWorkspaceCatalog {
    pub id: String,
}
impl Event for RefreshWorkspaceCatalog {}
#[derive(Clone, Debug)]
pub struct RefreshWorkspaceCatalogResult {
    pub id: String,
    pub result: Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>,
}
impl Event for RefreshWorkspaceCatalogResult {}
#[derive(Clone, Debug)]
pub struct QueryWorkspaceCatalog {
    pub id: String,
}
impl Event for QueryWorkspaceCatalog {}
#[derive(Clone, Debug)]
pub struct QueryWorkspaceCatalogResult {
    pub id: String,
    pub result: Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>,
}
impl Event for QueryWorkspaceCatalogResult {}
#[derive(Clone, Debug)]
pub struct StartRegisteredWorkspace {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
}
impl Event for StartRegisteredWorkspace {}
#[derive(Clone, Debug)]
pub struct StartRegisteredWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for StartRegisteredWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct AddWorkspaceMember {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
    pub member: WorkspaceMemberSpec,
}
impl Event for AddWorkspaceMember {}
#[derive(Clone, Debug)]
pub struct AddWorkspaceMemberResult {
    pub id: String,
    pub result: Result<WorkspaceCatalogEntry, WorkspaceError>,
}
impl Event for AddWorkspaceMemberResult {}
#[derive(Clone, Debug)]
pub struct RemoveWorkspaceMember {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
    pub member_id: ResourceID,
}
impl Event for RemoveWorkspaceMember {}
#[derive(Clone, Debug)]
pub struct RemoveWorkspaceMemberResult {
    pub id: String,
    pub result: Result<WorkspaceCatalogEntry, WorkspaceError>,
}
impl Event for RemoveWorkspaceMemberResult {}

#[derive(Clone, Debug)]
pub struct CreateWorkspace {
    pub id: String,
    pub name: String,
    /// Absolute, existing, canonical directory. Never created by the daemon.
    pub project_root: PathBuf,
}
impl Event for CreateWorkspace {}
#[derive(Clone, Debug)]
pub struct CreateWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceCatalogEntry, WorkspaceError>,
}
impl Event for CreateWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct RenameWorkspace {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
    pub new_name: String,
}
impl Event for RenameWorkspace {}
/// Only stopped workspaces are renamed; live workspaces must be stopped first.
#[derive(Clone, Debug)]
pub struct RenameWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceCatalogEntry, WorkspaceError>,
}
impl Event for RenameWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct DeleteWorkspace {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
}
impl Event for DeleteWorkspace {}
/// Only stopped workspaces are deleted; the directory is moved to recoverable storage.
#[derive(Clone, Debug)]
pub struct DeleteWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceDeletion, WorkspaceError>,
}
impl Event for DeleteWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct RenameWorkspaceMember {
    pub id: String,
    pub workspace_id: ResourceID,
    pub expected_revision: String,
    pub member_id: ResourceID,
    pub new_name: String,
}
impl Event for RenameWorkspaceMember {}
#[derive(Clone, Debug)]
pub struct RenameWorkspaceMemberResult {
    pub id: String,
    pub result: Result<WorkspaceCatalogEntry, WorkspaceError>,
}
impl Event for RenameWorkspaceMemberResult {}
