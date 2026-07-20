use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SandboxCommandRequested {
    pub command_id: String,
    pub command: String,
    pub current_dir: Option<PathBuf>,
    pub use_sandbox: bool,
}

impl SandboxCommandRequested {
    pub fn new(command_id: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            command_id: command_id.into(),
            command: command.into(),
            current_dir: None,
            use_sandbox: true,
        }
    }

    pub fn with_current_dir(mut self, current_dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(current_dir.into());
        self
    }

    pub fn without_sandbox(mut self) -> Self {
        self.use_sandbox = false;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCommandStarted {
    pub command_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCommandCompleted {
    pub command_id: String,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCommandFailed {
    pub command_id: String,
    pub kind: SandboxFailureKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxFailureKind {
    PermissionDenied,
    SpawnFailed,
    ExitNonZero,
}
