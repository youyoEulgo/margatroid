use crate::{WorkspaceDatabase, WorkspaceError};
use core_plugin::{Entity, Event};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

pub type WorkspaceReply<T> = Arc<Mutex<Option<oneshot::Sender<Result<T, WorkspaceError>>>>>;
pub type WorkspaceAgentReply<R, E> = Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>;

#[derive(Clone, Debug)]
pub struct CreateWorkspace {
    pub address: String,
    pub alias: String,
    pub project_root: PathBuf,
    pub reply: WorkspaceReply<Entity>,
}
impl Event for CreateWorkspace {}
#[derive(Clone, Debug)]
pub struct StartWorkspace {
    pub address: String,
    pub alias: String,
    pub project_root: PathBuf,
    pub database: WorkspaceDatabase,
    pub reply: Option<WorkspaceReply<Entity>>,
}
impl Event for StartWorkspace {}
#[derive(Clone, Debug)]
pub struct DeleteWorkspace {
    pub workspace: Entity,
    pub reply: WorkspaceReply<()>,
}
impl Event for DeleteWorkspace {}
#[derive(Debug)]
pub enum StartWorkspaceAgent<R, E> {
    Existing {
        workspace: Entity,
        node_name: String,
        reply: WorkspaceAgentReply<R, E>,
    },
    New {
        workspace: Entity,
        tool_path: PathBuf,
        arguments: serde_json::Value,
        reply: WorkspaceAgentReply<R, E>,
    },
}
impl<R, E> Clone for StartWorkspaceAgent<R, E> {
    fn clone(&self) -> Self {
        match self {
            Self::Existing {
                workspace,
                node_name,
                reply,
            } => Self::Existing {
                workspace: *workspace,
                node_name: node_name.clone(),
                reply: reply.clone(),
            },
            Self::New {
                workspace,
                tool_path,
                arguments,
                reply,
            } => Self::New {
                workspace: *workspace,
                tool_path: tool_path.clone(),
                arguments: arguments.clone(),
                reply: reply.clone(),
            },
        }
    }
}
impl<R: Send + 'static, E: Send + 'static> Event for StartWorkspaceAgent<R, E> {}
#[derive(Clone, Debug)]
pub struct ArchiveAgent {
    pub workspace: Entity,
    pub node_name: String,
    pub reply: WorkspaceReply<()>,
}
impl Event for ArchiveAgent {}
