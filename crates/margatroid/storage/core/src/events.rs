use crate::{StorageError, StorageOperation, StorageValue};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type StorageReply = Arc<Mutex<Option<oneshot::Sender<Result<StorageValue, StorageError>>>>>;

#[derive(Clone)]
pub struct StorageRequest {
    pub owner: Entity,
    pub operation: StorageOperation,
    pub cancel: CancellationToken,
    pub reply: StorageReply,
}
impl Event for StorageRequest {}
