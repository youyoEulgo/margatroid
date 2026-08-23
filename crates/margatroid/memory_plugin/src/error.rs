use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryErrorKind {
    InvalidPath,
    DirectoryCreateFailed,
    OpenFailed,
    SchemaFailed,
    ReadFailed,
    DecodeFailed,
    AgentNotAlive,
    AgentMemoryMissing,
    WriteFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryError {
    kind: MemoryErrorKind,
    message: String,
}

impl MemoryError {
    pub(crate) fn new(kind: MemoryErrorKind, message: impl Into<String>) -> Self {
        const MAX_MESSAGE_BYTES: usize = 512;
        const SUFFIX: &str = "...";

        let mut message = message.into();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut boundary = MAX_MESSAGE_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> MemoryErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for MemoryError {}
