use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    InvalidRequest,
    InvalidDefinition,
    Missing,
    Duplicate,
    StateRejected,
    Busy,
    MemberFailed,
    ThreadStartFailed,
    ThreadCloseFailed,
    CleanupFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    pub kind: WorkspaceErrorKind,
    pub message: String,
}
impl WorkspaceError {
    pub fn new(kind: WorkspaceErrorKind, message: impl fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for WorkspaceError {}
