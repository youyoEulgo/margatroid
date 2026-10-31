use app_runtime_plugin::RuntimeEventSender;
use async_runtime_plugin::AsyncTaskError;
use closure_plugin::WorldClosureSender;
use core_plugin::Entity;
use std::path::PathBuf;
use workspace_core::{MemberDiagnostic, WorkspaceDefinitions, WorkspaceError, WorkspaceErrorKind};

#[derive(Clone)]
pub(crate) struct WorkspaceFlowContext {
    pub id: String,
    pub workspace: Entity,
    pub events: RuntimeEventSender,
    pub path: PathBuf,
    pub definitions: WorkspaceDefinitions,
    pub world: WorldClosureSender,
}
#[derive(Clone)]
pub(crate) struct StartOutput {
    pub id: String,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct StopOutput {
    pub id: String,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct RestartOutput {
    pub id: String,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct FlowError {
    pub id: String,
    pub workspace: Option<Entity>,
    pub error: WorkspaceError,
    pub cleanup_completed: bool,
    pub restore_running: bool,
}
pub(crate) type StartError = FlowError;
pub(crate) type StopError = FlowError;
pub(crate) type RestartError = FlowError;
impl From<AsyncTaskError> for FlowError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            id: String::new(),
            workspace: None,
            error: WorkspaceError::new(WorkspaceErrorKind::CleanupFailed, error),
            cleanup_completed: false,
            restore_running: false,
        }
    }
}
