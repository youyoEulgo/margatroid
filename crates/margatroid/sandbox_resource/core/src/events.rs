use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use core_plugin::{Entity, Event};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::SandboxResourceError;

pub type ImportSandboxReply = Arc<Mutex<Option<oneshot::Sender<Result<(), SandboxResourceError>>>>>;

#[derive(Clone)]
pub struct ImportSandbox {
    pub owner: Entity,
    pub alias: String,
    pub path: PathBuf,
    pub cancel: CancellationToken,
    pub reply: ImportSandboxReply,
}

impl Event for ImportSandbox {}
