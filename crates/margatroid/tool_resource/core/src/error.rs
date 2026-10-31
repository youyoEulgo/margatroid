use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolResourceErrorKind {
    OwnerMissing,
    InvalidAlias,
    AliasMissing,
    InvalidPath,
    FileMissing,
    TooLarge,
    InvalidUtf8,
    InvalidDefinition,
    ReadFailed,
    Cancelled,
    WorkerFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResourceError {
    pub kind: ToolResourceErrorKind,
    pub path: Option<PathBuf>,
    pub message: String,
}

impl ToolResourceError {
    pub fn new(kind: ToolResourceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            path: None,
            message: message.into(),
        }
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl fmt::Display for ToolResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)?;
        if let Some(path) = &self.path {
            write!(formatter, " ({})", path.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for ToolResourceError {}
