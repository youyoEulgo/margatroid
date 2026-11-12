use crate::{WorkspaceError, WorkspaceOperationOutput};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
pub type WorkspaceOperationReply =
    Arc<Mutex<Option<oneshot::Sender<Result<WorkspaceOperationOutput, WorkspaceError>>>>>;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct StartWorkspace {
    pub id: String,
    pub path: PathBuf,
    pub reply: WorkspaceOperationReply,
}
impl Event for StartWorkspace {}
#[derive(Clone, Debug)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
    pub reply: WorkspaceOperationReply,
}
impl Event for StopWorkspace {}
#[derive(Clone, Debug)]
pub struct RestartWorkspace {
    pub id: String,
    pub path: PathBuf,
    pub reply: WorkspaceOperationReply,
}
impl Event for RestartWorkspace {}
