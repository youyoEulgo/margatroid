use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MclError {
    ParseFailed,
    InvalidAgentId,
    AgentMissing,
    DuplicateAgent,
    AgentRuntimeMissing,
    BlockMissing { assembly: String, block: String },
    InnerMissing { block: String, inner: String },
    RefBlockMissing { assembly: String, block: String },
    MergeMissing { block: String, merge: String },
    TypeMismatch,
    BindingMissing,
    InvalidCommand,
    ImportMissing,
    ImportFailed,
    ImportResponseMismatch,
    MessageSourceUnavailable,
    EffectAlreadyPending,
    EffectResponseMismatch,
    TurnMissing,
    TurnMismatch,
    MailboxFailed,
    InferenceFailed,
    ToolCallInvalid,
    RealtimeReadFailed,
    EffectInvalid,
    SourceReadFailed,
    InvalidResourceId,
    SourceTooLarge,
    SourceInvalidUtf8,
    ImportCycle,
    InvalidProgramKind,
}

impl fmt::Display for MclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for MclError {}
