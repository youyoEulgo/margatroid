use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclError {
    pub kind: MclErrorKind,
    pub message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MclErrorKind {
    ParseFailed,
    ArgumentMismatch,
    SourceInvalid,
    TargetMissing,
    Closed,
    AliasConflict,
    AliasMissing,
    ImportDenied,
    InvalidMessage,
    InvalidRange,
    ConfigPathInvalid,
    ConfigTypeMismatch,
    StorageFailed,
    CommitUnknown,
    CleanupFailed,
    EffectFailed,
    Cancelled,
    ReplyClosed,
}
impl MclError {
    pub fn new(kind: MclErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl fmt::Display for MclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for MclError {}
