use core_plugin::Entity;
use std::path::PathBuf;
use workspace_core::{WorkspaceError, WorkspaceReply};
#[derive(Clone)]
pub(crate) struct WorkspaceCleanupResult {
    pub workspace: Entity,
    pub directory: PathBuf,
    pub reply: WorkspaceReply<()>,
    pub result: Result<(), WorkspaceError>,
}

#[derive(Clone)]
pub(crate) struct WorkspaceWorkCompleted {
    pub token: std::sync::Arc<std::sync::Mutex<Option<workspace_core::WorkspaceWorkToken>>>,
}
impl core_plugin::Event for WorkspaceWorkCompleted {}
impl WorkspaceWorkCompleted {
    pub fn new(token: workspace_core::WorkspaceWorkToken) -> Self {
        Self {
            token: std::sync::Arc::new(std::sync::Mutex::new(Some(token))),
        }
    }
}
