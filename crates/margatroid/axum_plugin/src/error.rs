use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxumErrorKind {
    Installation,
    RoutesFrozen,
    Bind,
    Serve,
    Panicked,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxumError {
    pub kind: AxumErrorKind,
    pub message: String,
}

impl AxumError {
    pub(crate) fn new(kind: AxumErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for AxumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AxumError {}
