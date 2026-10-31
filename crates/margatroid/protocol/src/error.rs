use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorDto {
    pub kind: String,
    pub message: String,
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
            kind: kind.into(),
            message,
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
        write!(f, "{}: {}", self.0.kind, self.0.message)
    }
}
impl std::error::Error for ProtocolError {}
