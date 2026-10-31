use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use core_plugin::{Entity, Event};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::ToolResourceError;

pub type ImportToolReply = Arc<Mutex<Option<oneshot::Sender<Result<(), ToolResourceError>>>>>;

#[derive(Clone)]
pub struct ImportTool {
    pub owner: Entity,
    pub alias: String,
    pub path: PathBuf,
    pub cancel: CancellationToken,
    pub reply: ImportToolReply,
}

impl Event for ImportTool {}
