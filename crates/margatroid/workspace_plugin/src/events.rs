use core_plugin::{Entity, Event};
use margatroid_types::{WorkspaceDefinition, WorkspaceReference};

use crate::error::WorkspaceError;

#[derive(Clone, Debug)]
pub struct StartWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for StartWorkspaceResult {}

#[derive(Clone, Debug)]
pub struct ReloadWorkspace {
    pub id: String,
    pub workspace: Entity,
    pub definition: WorkspaceDefinition,
}
impl Event for ReloadWorkspace {}

#[derive(Clone, Debug)]
pub struct ReloadWorkspaceResult {
    pub id: String,
    pub previous: Entity,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for ReloadWorkspaceResult {}

#[derive(Clone, Debug)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
}
impl Event for StopWorkspace {}

#[derive(Clone, Debug)]
pub struct StopWorkspaceResult {
    pub id: String,
    pub workspace: Entity,
    pub result: Result<(), WorkspaceError>,
}
impl Event for StopWorkspaceResult {}

#[derive(Clone, Debug)]
pub struct StopWorkspaceByReference {
    pub id: String,
    pub workspace: WorkspaceReference,
}
impl Event for StopWorkspaceByReference {}

#[derive(Clone, Debug)]
pub struct StopWorkspaceByReferenceResult {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub result: Result<(), WorkspaceError>,
}
impl Event for StopWorkspaceByReferenceResult {}
