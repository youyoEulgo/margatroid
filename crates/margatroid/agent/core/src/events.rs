use crate::{AgentError, AgentHandle};
use core_plugin::{Entity, Event};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

pub type AgentOperationReply = Arc<Mutex<Option<oneshot::Sender<Result<AgentHandle, AgentError>>>>>;
#[derive(Clone, Debug)]
pub struct StartAgent<P: Send + Sync + 'static> {
    pub workspace: Entity,
    pub parent: Entity,
    pub task_id: String,
    pub address: String,
    pub project_root: PathBuf,
    pub ui_path: PathBuf,
    pub persistence: P,
    pub reply: AgentOperationReply,
}
impl<P: Send + Sync + 'static> Event for StartAgent<P> {}
pub type AgentStopReply = Arc<Mutex<Option<oneshot::Sender<Result<(), AgentError>>>>>;
#[derive(Clone, Debug)]
pub struct StopAgent {
    pub address: String,
    pub reply: AgentStopReply,
}
impl Event for StopAgent {}
