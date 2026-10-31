use async_runtime_plugin::AsyncTaskError;
use core_plugin::Entity;
use std::sync::{Arc, Mutex};
use storage_core::{AgentStorage, StorageError, StorageErrorKind, StorageOperation, StorageValue};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) type Reply = Arc<Mutex<Option<oneshot::Sender<Result<StorageValue, StorageError>>>>>;
pub(crate) struct StorageContext {
    pub owner: Entity,
    pub storage: AgentStorage,
    pub operation: StorageOperation,
    pub cancel: CancellationToken,
    pub reply: Reply,
    pub pending: PendingReply,
}
pub(crate) struct PendingReply {
    pub reply: Reply,
    pub active: bool,
}
impl Drop for PendingReply {
    fn drop(&mut self) {
        if self.active {
            if let Some(sender) = self.reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = sender.send(Err(StorageError::new(
                    StorageErrorKind::Cancelled,
                    "storage task stopped before worker submission",
                )));
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct StorageOutput {
    pub owner: Entity,
    pub reply: Reply,
    pub value: StorageValue,
}
#[derive(Clone)]
pub(crate) struct StorageTaskError {
    pub owner: Option<Entity>,
    pub reply: Option<Reply>,
    pub error: StorageError,
}
impl From<AsyncTaskError> for StorageTaskError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            owner: None,
            reply: None,
            error: StorageError::new(StorageErrorKind::WorkerFailed, error),
        }
    }
}
impl Drop for StorageOutput {
    fn drop(&mut self) {
        if let Some(sender) = self.reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sender.send(Ok(self.value.clone()));
        }
    }
}
impl Drop for StorageTaskError {
    fn drop(&mut self) {
        if let Some(reply) = &self.reply {
            if let Some(sender) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = sender.send(Err(self.error.clone()));
            }
        }
    }
}
