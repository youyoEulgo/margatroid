use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    AgentMissing,
    ResourceMapMissing,
    InvalidResource,
    ResourceUnavailable,
    RegistrationFailed,
    ToolCallMissing,
    InvalidDefinition,
    ProviderMissing,
    ResourceResolutionFailed,
    AgentNotAlive,
    ToolEnvironmentMissing,
    ToolPluginMissing,
    ToolAlreadyRegistered,
    DuplicateResource,
    InvalidRequest,
    InvalidArguments,
    ExecutionFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        const LIMIT: usize = 512;
        let mut message = message.into();
        if message.len() > LIMIT {
            let mut boundary = LIMIT - 3;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolError {}
