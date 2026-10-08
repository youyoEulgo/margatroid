use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    InvalidRequest,
    InvalidDefinition,
    InvalidProjectRoot,
    InvalidAgentImagesRoot,
    DuplicateWorkspace,
    DuplicateResource,
    WorkspaceNotAlive,
    WorkspaceNotReady,
    WorkspaceMismatch,
    AgentImageLoadFailed,
    AgentImageComponentsMissing,
    InferenceSetupFailed,
    MemorySetupFailed,
    ResourceSetupFailed,
    AgentCreateFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    kind: WorkspaceErrorKind,
    message: String,
}

impl WorkspaceError {
    pub fn new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind.clone()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WorkspaceError {}

impl From<resource_id_plugin::ResourceIdLookupError> for WorkspaceError {
    fn from(error: resource_id_plugin::ResourceIdLookupError) -> Self {
        let kind = match &error {
            resource_id_plugin::ResourceIdLookupError::Missing { .. } => {
                WorkspaceErrorKind::WorkspaceNotAlive
            }
            resource_id_plugin::ResourceIdLookupError::Duplicate { .. } => {
                WorkspaceErrorKind::DuplicateResource
            }
            resource_id_plugin::ResourceIdLookupError::PluginMissing => {
                WorkspaceErrorKind::InvalidRequest
            }
        };
        Self::new(kind, error.to_string())
    }
}
