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
pub enum ToolExecuteRequest {
    Registered {
        id: String,
        agent: Entity,
        turn: Option<String>,
        alias: String,
        call_id: String,
        arguments: Value,
        reply: ToolExecuteReply,
    },
    Path {
        initiator: Entity,
        path: std::path::PathBuf,
        arguments: Value,
        reply: ToolExecuteReply,
    },
}

impl Event for ToolExecuteRequest {}
