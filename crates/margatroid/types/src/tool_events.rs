use crate::message_types::{AgentMessage, ToolCall};
use crate::resource_components::CallableResource;
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolErrorKind {
    InvalidRequest,
    AgentMissing,
    AgentNotAlive,
    ResourceMissing,
    UnsupportedResource,
    UnsupportedHandoff,
    InvalidArguments,
    InvalidDefinition,
    ScriptReadFailed,
    ExecutionBusy,
    SandboxUnavailable,
    SubmitFailed,
    RunnerStartFailed,
    RunnerProtocolFailed,
    ExecutionFailed,
    OutputTooLarge,
    Timeout,
    Cancelled,
    TaskPanicked,
    CleanupFailed,
    PreparationFailed,
    ResponseLost,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}
impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().chars().take(512).collect(),
        }
    }
}
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for ToolError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecuteResponse {
    pub message: AgentMessage,
}
pub type ToolExecuteReply =
    Arc<Mutex<Option<oneshot::Sender<Result<ToolExecuteResponse, ToolError>>>>>;
#[derive(Clone, Debug)]
pub struct ToolExecuteRequest {
    pub id: String,
    pub agent: Entity,
    pub resource: CallableResource,
    pub call: ToolCall,
    pub turn: Option<String>,
    pub cancel: CancellationToken,
    pub reply: ToolExecuteReply,
}
impl Event for ToolExecuteRequest {}
