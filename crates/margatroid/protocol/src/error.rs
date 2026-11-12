use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolErrorDto {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl ProtocolErrorDto {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        let mut message = message.into();
        let mut end = message.len().min(512);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        Self {
            code: kind.into(),
            message,
            details: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError(pub ProtocolErrorDto);
impl ProtocolError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self(ProtocolErrorDto::new(kind, message))
    }
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.code, self.0.message)
    }
}
impl std::error::Error for ProtocolError {}
