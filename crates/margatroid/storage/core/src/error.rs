use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageErrorKind {
    OwnerMissing,
    Closed,
    InvalidRequest,
    InvalidRecord,
    KeyConflict,
    SchemaMismatch,
    LifecycleMismatch,
    BlockMissing,
    FieldMissing,
    InvalidRange,
    ConfigInvalid,
    ConfigPathInvalid,
    ConfigTypeMismatch,
    ReadFailed,
    WriteFailed,
    Cancelled,
    CommitUnknown,
    CleanupFailed,
    WorkerFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageError {
    pub kind: StorageErrorKind,
    pub message: String,
}

impl StorageError {
    pub fn new(kind: StorageErrorKind, message: impl fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
}
impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for StorageError {}
