use agent_core::{AgentError, AgentErrorKind, AgentStartParameters, AgentState};
use app_runtime_plugin::RuntimeEventSender;
use async_runtime_plugin::AsyncTaskError;
use closure_plugin::WorldClosureSender;
use core_plugin::Entity;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

pub(crate) type Reply = Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>;
#[derive(Clone)]
pub(crate) struct FlowContext {
    pub id: String,
    pub agent: Entity,
    pub workspace: Entity,
    pub state: AgentState,
    pub events: RuntimeEventSender,
    pub world: WorldClosureSender,
    pub parameters: Option<AgentStartParameters>,
    pub driver_root: PathBuf,
    pub reply: Reply,
}
#[derive(Clone)]
pub(crate) struct StartOutput {
    pub id: String,
    pub agent: Entity,
    pub reply: Reply,
}
#[derive(Clone)]
pub(crate) struct StopOutput {
    pub id: String,
    pub agent: Entity,
    pub reply: Reply,
}
#[derive(Clone)]
pub(crate) struct RestartOutput {
    pub id: String,
    pub agent: Entity,
    pub reply: Reply,
}
#[derive(Clone)]
pub(crate) struct FlowError {
    pub id: String,
    pub agent: Option<Entity>,
    pub reply: Option<Reply>,
    pub error: AgentError,
    pub cleanup_completed: bool,
}
pub(crate) type StartError = FlowError;
pub(crate) type StopError = FlowError;
pub(crate) type RestartError = FlowError;
impl From<AsyncTaskError> for FlowError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            id: String::new(),
            agent: None,
            reply: None,
            error: AgentError::new(AgentErrorKind::CleanupFailed, error.to_string()),
            cleanup_completed: false,
        }
    }
}
pub(crate) struct StartFailure {
    pub error: AgentError,
    pub uncertain: bool,
}
impl From<AgentError> for StartFailure {
    fn from(error: AgentError) -> Self {
        let uncertain = matches!(
            error.kind,
            AgentErrorKind::CleanupFailed | AgentErrorKind::ReplyClosed
        );
        Self { error, uncertain }
    }
}
