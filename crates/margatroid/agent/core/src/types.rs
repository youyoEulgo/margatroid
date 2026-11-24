use crate::{AgentError, AgentErrorKind};
use core_plugin::Entity;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Starting,
    Running,
    Closing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentReadyState {
    Pending,
    Ready,
    Closed,
}
pub type AgentCloseFuture = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>;
pub type AgentReply = oneshot::Sender<Result<AgentHandle, AgentError>>;

#[derive(Clone)]
pub struct AgentHandle {
    entity: Entity,
    ready: watch::Receiver<AgentReadyState>,
    close: Arc<dyn Fn() -> AgentCloseFuture + Send + Sync>,
}
impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}
impl AgentHandle {
    pub fn new(
        entity: Entity,
        ready: watch::Receiver<AgentReadyState>,
        close: impl Fn() -> AgentCloseFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            entity,
            ready,
            close: Arc::new(close),
        }
    }
    pub fn entity(&self) -> Entity {
        self.entity
    }
    pub async fn wait_ready(&self) -> Result<(), AgentError> {
        let mut ready = self.ready.clone();
        loop {
            match *ready.borrow_and_update() {
                AgentReadyState::Ready => return Ok(()),
                AgentReadyState::Closed => {
                    return Err(AgentError::new(
                        AgentErrorKind::StateRejected,
                        "Agent closed before ready",
                    ));
                }
                AgentReadyState::Pending => {}
            }
            ready.changed().await.map_err(|_| {
                AgentError::new(AgentErrorKind::ReplyClosed, "Agent ready source closed")
            })?;
        }
    }
    pub fn close(&self) -> AgentCloseFuture {
        (self.close)()
    }
}

#[derive(Debug, Default)]
pub struct AgentWakeSlot {
    sender: Option<(u64, oneshot::Sender<()>)>,
    pending_wake: bool,
    waiter: Option<u64>,
    next_waiter: u64,
    closed: bool,
}
impl AgentWakeSlot {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn wake(&mut self) -> Result<(), AgentError> {
        if self.closed {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "Agent wake slot closed",
            ));
        }
        if let Some((_, sender)) = self.sender.take() {
            if sender.send(()).is_err() {
                self.pending_wake = true;
            }
        } else {
            self.pending_wake = true;
        }
        Ok(())
    }
    pub async fn wait(slot: Arc<Mutex<Self>>) -> Result<(), AgentError> {
        let (id, rx) = {
            let mut state = slot.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed || state.waiter.is_some() {
                return Err(AgentError::new(
                    AgentErrorKind::StateRejected,
                    "Agent wake slot closed or already waiting",
                ));
            }
            if state.pending_wake {
                state.pending_wake = false;
                return Ok(());
            }
            state.next_waiter = state.next_waiter.checked_add(1).ok_or_else(|| {
                AgentError::new(AgentErrorKind::StateRejected, "wake identity exhausted")
            })?;
            let id = state.next_waiter;
            let (tx, rx) = oneshot::channel();
            state.waiter = Some(id);
            state.sender = Some((id, tx));
            (id, rx)
        };
        let _guard = WakeGuard { slot, id };
        rx.await
            .map_err(|_| AgentError::new(AgentErrorKind::StateRejected, "Agent wake slot closed"))
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.pending_wake = false;
        self.sender = None;
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    pub fn is_waiting(&self) -> bool {
        self.waiter.is_some()
    }
}
struct WakeGuard {
    slot: Arc<Mutex<AgentWakeSlot>>,
    id: u64,
}
impl Drop for WakeGuard {
    fn drop(&mut self) {
        let mut state = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if state.waiter == Some(self.id) {
            state.waiter = None;
            if state.sender.as_ref().is_some_and(|(id, _)| *id == self.id) {
                state.sender = None;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum AgentUpdateKind {
    Installed,
    Changed,
    SourcesInstalled { ui_path: Option<PathBuf> },
    SourcesClosing,
    Removed,
}
