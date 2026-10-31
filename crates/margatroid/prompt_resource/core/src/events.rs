use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use core_plugin::{Entity, Event};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::PromptResourceError;

pub type ImportPromptReply = Arc<Mutex<Option<oneshot::Sender<Result<(), PromptResourceError>>>>>;

#[derive(Clone)]
pub struct ImportPrompt {
    pub owner: Entity,
    pub alias: String,
    pub path: PathBuf,
    pub cancel: CancellationToken,
    pub reply: ImportPromptReply,
}

impl Event for ImportPrompt {}
