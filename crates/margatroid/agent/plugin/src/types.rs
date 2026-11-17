use agent_core::{AgentError, AgentErrorKind, AgentStartParameters, AgentState};
use async_runtime_plugin::AsyncTaskError;
use closure_plugin::WorldClosureSender;
use core_plugin::Entity;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub(crate) type Reply = Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>;
#[derive(Clone)]
pub(crate) struct FlowContext {
    pub id: String,
    pub agent: Entity,
    pub workspace: Entity,
    pub state: AgentState,
    pub world: WorldClosureSender,
    pub parameters: Option<AgentStartParameters>,
    pub reply: Reply,
    pub startup: Arc<StartupCompletion>,
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
pub(crate) struct FlowError {
    pub id: String,
    pub agent: Option<Entity>,
    pub reply: Option<Reply>,
    pub error: AgentError,
    pub cleanup_completed: bool,
    pub state: Option<AgentState>,
}
pub(crate) type StartError = FlowError;
pub(crate) type StopError = FlowError;
impl From<AsyncTaskError> for FlowError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            id: String::new(),
            agent: None,
            reply: None,
            error: AgentError::new(AgentErrorKind::CleanupFailed, error.to_string()),
            cleanup_completed: false,
            state: None,
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
pub(crate) struct StartupBarrier(pub Arc<StartupCompletion>);
impl core_plugin::Component for StartupBarrier {}
