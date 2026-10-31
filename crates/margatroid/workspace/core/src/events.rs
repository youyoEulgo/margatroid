use crate::{WorkspaceError, WorkspaceOperationOutput};
use core_plugin::{Entity, Event};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartWorkspace {
    pub id: String,
    pub path: PathBuf,
}
impl Event for StartWorkspace {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
}
impl Event for StopWorkspace {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartWorkspace {
    pub id: String,
    pub path: PathBuf,
}
impl Event for RestartWorkspace {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceOperationOutput, WorkspaceError>,
}
impl Event for StartWorkspaceResult {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceOperationOutput, WorkspaceError>,
}
impl Event for StopWorkspaceResult {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartWorkspaceResult {
    pub id: String,
    pub result: Result<WorkspaceOperationOutput, WorkspaceError>,
}
impl Event for RestartWorkspaceResult {}
