use crate::workspace_components::{WorkspaceDefinition, WorkspaceError};
use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;

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
