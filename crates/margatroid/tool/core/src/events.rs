use crate::{ToolError, ToolExecuteResponse};
use core_plugin::{Entity, Event};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type InitToolTasksReply = Arc<Mutex<Option<oneshot::Sender<Result<(), ToolError>>>>>;
pub type ToolExecuteReply =
    Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>;

#[derive(Clone, Debug)]
pub struct InitToolTasks {
    pub agent: Entity,
    pub reply: InitToolTasksReply,
}

impl Event for InitToolTasks {}

#[derive(Clone, Debug)]
pub struct ToolExecuteRequest {
    pub id: String,
    pub agent: Entity,
    pub alias: String,
    pub call_id: String,
    pub arguments: Value,
    pub reply: ToolExecuteReply,
}

impl Event for ToolExecuteRequest {}
