use crate::{AgentError, AgentErrorKind, AgentReadyState, AgentState, AgentWakeSlot};
use core_plugin::{Component, Entity};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Debug)]
pub struct Agent {
    pub(crate) workspace: Entity,
    pub(crate) parent: Entity,
    pub(crate) task_id: String,
    pub(crate) instance: String,
    pub(crate) project_root: PathBuf,
    pub(crate) state: AgentState,
    pub(crate) turn: Option<String>,
    pub(crate) ready: watch::Sender<AgentReadyState>,
    pub(crate) wake: Arc<Mutex<AgentWakeSlot>>,
}
impl Agent {
    pub fn new(workspace: Entity, parent: Entity, task_id: String, project_root: PathBuf) -> Self {
        Self {
            workspace,
            parent,
            task_id,
            instance: crate::unique_id("agent"),
            project_root,
            state: AgentState::Starting,
            turn: None,
            ready: watch::channel(AgentReadyState::Pending).0,
            wake: Arc::new(Mutex::new(AgentWakeSlot::new())),
        }
    }
    pub fn workspace(&self) -> Entity {
        self.workspace
    }
    pub fn parent(&self) -> Entity {
        self.parent
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn task_owner(&self) -> (Entity, &str) {
        (self.parent, &self.task_id)
    }
    pub fn instance(&self) -> &str {
        &self.instance
    }
    pub fn project_root(&self) -> &std::path::Path {
        &self.project_root
    }
    pub fn state(&self) -> AgentState {
        self.state
    }
    pub fn turn(&self) -> &Option<String> {
        &self.turn
    }
    pub fn ready_receiver(&self) -> watch::Receiver<AgentReadyState> {
        self.ready.subscribe()
    }
    pub fn wake_slot(&self) -> Arc<Mutex<AgentWakeSlot>> {
        self.wake.clone()
    }
    pub(crate) fn mark_ready(&mut self) -> Result<bool, AgentError> {
        match self.state {
            AgentState::Starting => {
                self.state = AgentState::Running;
                self.ready.send_replace(AgentReadyState::Ready);
                Ok(true)
            }
            AgentState::Running => Ok(false),
            AgentState::Closing => Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "Agent is closing",
            )),
        }
    }
    pub(crate) fn set_turn(&mut self, turn: Option<String>) -> Result<bool, AgentError> {
        if self.state != AgentState::Running || turn.as_ref().is_some_and(|v| v.trim().is_empty()) {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn requires a running Agent",
            ));
        }
        let changed = self.turn != turn;
        self.turn = turn;
        Ok(changed)
    }
    pub(crate) fn begin_close(&mut self) -> bool {
        let changed = self.state != AgentState::Closing || self.turn.is_some();
        self.state = AgentState::Closing;
        self.turn = None;
        self.ready.send_replace(AgentReadyState::Closed);
        self.wake.lock().unwrap_or_else(|e| e.into_inner()).close();
        changed
    }
}
impl Drop for Agent {
    fn drop(&mut self) {
        self.ready.send_replace(AgentReadyState::Closed);
        self.wake.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
}
impl Component for Agent {}
