use agent_core::{AgentError, AgentObservationWait};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) enum CloseStatus {
    Idle,
    Running,
    Failed(AgentError),
    Closed,
}

pub(crate) struct CloseControl {
    pub status: Mutex<CloseStatus>,
    pub result: watch::Sender<CloseStatus>,
    // Observer waits survive a cancelled waiter and are drained by the independent supervisor.
    pub observations: Mutex<Vec<AgentObservationWait>>,
}
impl CloseControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(CloseStatus::Idle),
            result: watch::channel(CloseStatus::Idle).0,
            observations: Mutex::new(Vec::new()),
        })
    }
}
