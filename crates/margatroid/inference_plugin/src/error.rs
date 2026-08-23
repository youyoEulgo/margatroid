use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceErrorKind {
    InvalidModelId,
    ConfigPathUnavailable,
    ConfigReadFailed,
    ConfigDecodeFailed,
    DuplicateModelId,
    InvalidModelRoute,
    UnsupportedApiType,
    InvalidCommand,
    AgentNotAlive,
    InferenceSnapshotMissing,
    ModelRouteNotFound,
    InvalidParameters,
    InvalidMessages,
    InvalidToolDefinitions,
    UnsupportedInput,
    RequestBuildFailed,
    RequestFailed,
    ResponseStatus,
    ResponseDecodeFailed,
    ResponseEncodeFailed,
    ResponseIncomplete,
    TaskPanicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceError {
    kind: InferenceErrorKind,
    message: String,
    status: Option<u16>,
}

impl InferenceError {
    pub fn new(kind: InferenceErrorKind, message: impl Into<String>) -> Self {
        Self::with_status(kind, None, message)
    }

    pub fn with_status(
        kind: InferenceErrorKind,
        status: Option<u16>,
        message: impl Into<String>,
    ) -> Self {
        let mut message = message.into();
        const SUFFIX: &str = "...";
        const MAX_BYTES: usize = 512;
        if message.len() > MAX_BYTES {
            let mut boundary = MAX_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self {
            kind,
            message,
            status,
        }
    }

    pub fn kind(&self) -> InferenceErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn status(&self) -> Option<u16> {
        self.status
    }

    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(
                formatter,
                "{:?} (HTTP {status}): {}",
                self.kind, self.message
            ),
            None => write!(formatter, "{:?}: {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for InferenceError {}
