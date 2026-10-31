use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentErrorKind {
    InvalidRequest,
    AgentMissing,
    StateRejected,
    UnfinishedTasks,
    StartFailed,
    StopFailed,
    RestartFailed,
    CleanupFailed,
    ReplyClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
}

impl AgentError {
    pub fn new(kind: AgentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AgentError {}
