use crate::{MclError, MclInstruction, MclValue};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type MclReply = Arc<Mutex<Option<oneshot::Sender<Result<MclValue, MclError>>>>>;

#[derive(Clone, Debug)]
pub struct MclCommand {
    pub id: String,
    pub source: Entity,
    pub source_instance: Option<String>,
    pub target: Entity,
    pub instruction: MclInstruction,
    pub cancel: CancellationToken,
    pub reply: MclReply,
}
impl Event for MclCommand {}
