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
    ImportMissing(String),
    ImportFailed,
    ImportResponseMismatch,
    MessageSourceUnavailable,
    EffectAlreadyPending,
    EffectResponseMismatch,
    EffectFailed(String),
    TurnMissing,
    TurnMismatch,
    MailboxFailed,
    InferenceFailed,
    ToolCallInvalid,
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

#[cfg(test)]
mod tests {
    use super::MclError;
    use resource_id_plugin::{ResourceIdLookupError, ResourceRef};

    #[test]
    fn lookup_errors_preserve_missing_and_duplicate() {
        let id = ResourceRef::parse("agent:local/demo").unwrap();
        assert_eq!(
            MclError::from(ResourceIdLookupError::Missing { id: id.clone() }),
            MclError::AgentMissing
        );
        assert_eq!(
            MclError::from(ResourceIdLookupError::Duplicate {
                id,
                entities: Vec::new()
            }),
            MclError::DuplicateAgent
        );
        assert_eq!(
            MclError::from(ResourceIdLookupError::PluginMissing),
            MclError::AgentRuntimeMissing
        );
    }
}

impl From<resource_id_plugin::ResourceIdLookupError> for MclError {
    fn from(error: resource_id_plugin::ResourceIdLookupError) -> Self {
        match error {
            resource_id_plugin::ResourceIdLookupError::Missing { .. } => Self::AgentMissing,
            resource_id_plugin::ResourceIdLookupError::Duplicate { .. } => Self::DuplicateAgent,
            resource_id_plugin::ResourceIdLookupError::PluginMissing => Self::AgentRuntimeMissing,
        }
    }
}
