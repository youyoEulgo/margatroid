use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolErrorKind {
    InvalidRequest,
    AgentMissing,
    TaskMissing,
    DuplicateTask,
    AliasMissing,
    InvalidArguments,
    InvalidDefinition,
    ReadFailed,
    Closed,
    SubmitFailed,
    SandboxUnavailable,
    RunnerStartFailed,
    RunnerProtocolFailed,
    ExecutionFailed,
    OutputTooLarge,
    Timeout,
    Cancelled,
    TaskPanicked,
    CleanupFailed,
    AlreadyReturned,
    MissingReturn,
    InputClosed,
    InputFull,
    OwnerGone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolError {}
