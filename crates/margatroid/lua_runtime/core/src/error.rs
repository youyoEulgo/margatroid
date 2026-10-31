use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaRuntimeErrorKind {
    InvalidRequest,
    InstanceNotFound,
    InstanceMismatch,
    AlreadyReady,
    SourceTooLarge,
    EnvironmentFailed,
    CompileFailed,
    ExecutionFailed,
    StartupTimeout,
    Cancelled,
    CleanupFailed,
    StopAlreadyWaiting,
    EndNotificationLost,
    AlreadyWaiting,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeError {
    pub kind: LuaRuntimeErrorKind,
    pub description: String,
}

impl LuaRuntimeError {
    pub fn new(kind: LuaRuntimeErrorKind, description: impl Into<String>) -> Self {
        Self {
            kind,
            description: description.into(),
        }
    }
}

impl fmt::Display for LuaRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.description)
    }
}

impl std::error::Error for LuaRuntimeError {}
