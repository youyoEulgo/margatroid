use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceError {
    pub kind: InferenceErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InferenceErrorKind {
    InvalidRequest,
    AgentMissing,
    InferenceBusy,
    InvalidParameters,
    InvalidMessages,
    ToolMissing,
    InvalidToolDefinition,
    ModelRouteMissing,
    InvalidModelRoute,
    UnsupportedFormat,
    RequestFailed,
    RequestBodyTooLarge,
    ResponseStatus,
    ResponseInvalid,
    ResponseIncomplete,
    ResponseTooLarge,
    Timeout,
    Cancelled,
    ReplyClosed,
    TaskPanicked,
    CleanupFailed,
}
impl InferenceError {
    pub fn new(kind: InferenceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: None,
        }
    }
}
impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for InferenceError {}
