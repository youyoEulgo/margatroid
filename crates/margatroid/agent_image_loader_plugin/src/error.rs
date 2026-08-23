use std::fmt;

use async_runtime_plugin::AsyncTaskError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentImageLoadErrorKind {
    InvalidRoot,
    InvalidRequest,
    NotFound,
    InvalidLayout,
    SymlinkNotAllowed,
    LimitExceeded,
    SourceChanged,
    ManifestReadFailed,
    ManifestDecodeFailed,
    UnsupportedSchema,
    InvalidModelConfig,
    PromptReadFailed,
    DuplicateDependency,
    InvalidResourceName,
    BaseMclLoadFailed,
    TaskPanicked,
}

#[derive(Clone, Debug)]
pub struct AgentImageLoadError {
    kind: AgentImageLoadErrorKind,
    message: String,
}

impl AgentImageLoadError {
    pub(crate) fn new(kind: AgentImageLoadErrorKind, message: impl Into<String>) -> Self {
        const MAX_MESSAGE_BYTES: usize = 512;

        let mut message = message.into();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut boundary = MAX_MESSAGE_BYTES - 3;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
        }
        Self { kind, message }
    }

    pub(crate) fn invalid_root(message: impl Into<String>) -> Self {
        Self::new(AgentImageLoadErrorKind::InvalidRoot, message)
    }

    pub fn kind(&self) -> AgentImageLoadErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AgentImageLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AgentImageLoadError {}

pub(crate) struct AgentImageTaskError {
    pub(crate) source: AsyncTaskError,
}

impl From<AsyncTaskError> for AgentImageTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}
