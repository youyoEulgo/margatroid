use crate::execution_components::ExecutionError;
use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentErrorKind {
    InvalidRequest,
    InvalidDefinition,
    AliasConflict,
    DuplicateAgent,
    AgentNotAlive,
    AgentBusy,
    AgentStopping,
    WorkspaceNotAlive,
    ImageNotLoaded,
    ResourceSetupFailed,
    HistorySetupFailed,
    HistoryWriteFailed,
    DriverStartFailed,
    DriverStopFailed,
    CleanupFailed,
    ResourceResolutionFailed,
    InvalidResourceID,
    SandboxNotLoaded,
    ExecutionBusy,
    BlockOperationFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
}
impl AgentError {
    pub fn new(kind: AgentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for AgentError {}
impl From<ExecutionError> for AgentError {
    fn from(error: ExecutionError) -> Self {
        let kind = match &error {
            ExecutionError::CleanupFailed { .. } => AgentErrorKind::CleanupFailed,
            ExecutionError::Closed => AgentErrorKind::AgentStopping,
            _ => AgentErrorKind::ExecutionBusy,
        };
        Self::new(kind, error.to_string())
    }
}
#[derive(Clone, Debug)]
pub struct AgentStartDefinition {
    pub project_root: PathBuf,
    pub home_root: PathBuf,
    pub history_path: PathBuf,
}
#[derive(Clone, Debug)]
pub struct StartAgent {
    pub id: String,
    pub resource_id: ResourceID,
    pub workspace: Entity,
    pub image: ResourceID,
    pub definition: AgentStartDefinition,
}
impl Event for StartAgent {}
#[derive(Clone, Debug)]
pub struct StartAgentResult {
    pub id: String,
    pub result: Result<Entity, AgentError>,
}
impl Event for StartAgentResult {}
#[derive(Clone, Debug)]
pub struct StopAgent {
    pub id: String,
    pub agent: Entity,
}
impl Event for StopAgent {}
#[derive(Clone, Debug)]
pub struct StopAgentResult {
    pub id: String,
    pub result: Result<Entity, AgentError>,
}
impl Event for StopAgentResult {}

#[derive(Clone, Debug)]
pub struct AgentException {
    pub exception_id: String,
    pub agent: Entity,
    pub resource_id: ResourceID,
    pub workspace: ResourceID,
    pub instance: crate::lua_runtime_components::DriverLuaVmInstance,
    pub error: AgentError,
    pub cleanup_completed: bool,
}
impl Event for AgentException {}
#[derive(Clone, Debug)]
pub struct AgentExceptionResult {
    pub exception_id: String,
    pub agent: Entity,
    pub resource_id: ResourceID,
    pub workspace: ResourceID,
    pub instance: crate::lua_runtime_components::DriverLuaVmInstance,
    pub error: Option<AgentError>,
    pub result: Result<(), AgentError>,
    pub execution_ended: bool,
    pub cleanup_completed: bool,
}
impl Event for AgentExceptionResult {}
