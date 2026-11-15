use app_runtime_plugin::RuntimeEventSender;
use async_runtime_plugin::AsyncTaskError;
use closure_plugin::WorldClosureSender;
use core_plugin::Entity;
use std::path::PathBuf;
use workspace_core::{
    MemberDiagnostic, WorkspaceDefinitions, WorkspaceError, WorkspaceErrorKind,
    WorkspaceOperationReply,
};

#[derive(Clone)]
pub(crate) struct WorkspaceFlowContext {
    pub id: String,
    pub reply: WorkspaceOperationReply,
    pub workspace: Entity,
    pub events: RuntimeEventSender,
    pub path: PathBuf,
    pub definitions: WorkspaceDefinitions,
    pub program_root: PathBuf,
    pub world: WorldClosureSender,
    pub state: workspace_core::WorkspaceState,
    pub startup: std::sync::Arc<StartupCompletion>,
}
#[derive(Clone)]
pub(crate) struct StartOutput {
    pub reply: WorkspaceOperationReply,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct StopOutput {
    pub reply: WorkspaceOperationReply,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct RestartOutput {
    pub reply: WorkspaceOperationReply,
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}
#[derive(Clone)]
pub(crate) struct FlowError {
    pub reply: WorkspaceOperationReply,
    pub workspace: Option<Entity>,
    pub error: WorkspaceError,
    pub cleanup_completed: bool,
    pub state: Option<workspace_core::WorkspaceState>,
}
pub(crate) type StartError = FlowError;
pub(crate) type StopError = FlowError;
pub(crate) type RestartError = FlowError;
impl From<AsyncTaskError> for FlowError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            reply: std::sync::Arc::new(std::sync::Mutex::new(None)),
            workspace: None,
            error: WorkspaceError::new(WorkspaceErrorKind::CleanupFailed, error),
            cleanup_completed: false,
            state: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct StartupCompletion {
    done: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
}
impl StartupCompletion {
    pub fn finish(&self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
        self.changed.notify_waiters();
    }
    pub async fn wait(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.done.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}
pub(crate) struct StartupBarrier(pub std::sync::Arc<StartupCompletion>);
impl core_plugin::Component for StartupBarrier {}
