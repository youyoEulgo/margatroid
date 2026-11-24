use std::{fmt, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    InvalidRequest,
    Missing,
    Busy,
    Closed,
    DriverMismatch,
    ParentMismatch,
    DuplicateNode,
    UnregisteredDirectory,
    DatabaseFailed,
    CloseFailed,
    ArchivePending,
    RecoveryFailed,
    DeleteFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    pub kind: WorkspaceErrorKind,
    pub message: String,
    pub phase: Option<String>,
    pub managed_directory: Option<PathBuf>,
    pub entity_removed: bool,
}
impl WorkspaceError {
    pub fn new(kind: WorkspaceErrorKind, message: impl fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
            phase: None,
            managed_directory: None,
            entity_removed: false,
        }
    }
    pub fn at_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }
    pub fn with_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.managed_directory = Some(path.into());
        self
    }
    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)?;
        if let Some(phase) = &self.phase {
            write!(f, " ({phase})")?;
        }
        Ok(())
    }
}
impl std::error::Error for WorkspaceError {}
