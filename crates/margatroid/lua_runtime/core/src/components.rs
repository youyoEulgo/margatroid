use crate::{DriverLuaVmInstance, DriverWakeSlot, LuaRuntimeError};
use core_plugin::Component;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type DriverReadySender = Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>;

#[derive(Debug)]
pub struct DriverLuaVm {
    pub instance: DriverLuaVmInstance,
    pub wake: Arc<Mutex<DriverWakeSlot>>,
    pub ready: DriverReadySender,
    pub ended: Option<oneshot::Receiver<Result<(), LuaRuntimeError>>>,
    pub cancel: CancellationToken,
}

impl Component for DriverLuaVm {}

#[derive(Debug)]
pub struct AgentWaitSlot(pub Arc<Mutex<DriverWakeSlot>>);
impl Default for AgentWaitSlot {
    fn default() -> Self {
        Self(DriverWakeSlot::shared())
    }
}
impl Component for AgentWaitSlot {}
impl Drop for AgentWaitSlot {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceLuaTask {
    pub workspace: core_plugin::Entity,
    pub name: String,
}
impl Component for WorkspaceLuaTask {}

/// Observes one actual execution; dropping the Agent session unlinks the monitor.
pub struct LuaAgentMonitor {
    unlink: Option<Box<dyn FnOnce() + Send + Sync>>,
}
impl LuaAgentMonitor {
    pub fn new(unlink: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            unlink: Some(Box::new(unlink)),
        }
    }
}
impl Component for LuaAgentMonitor {}
impl Drop for LuaAgentMonitor {
    fn drop(&mut self) {
        if let Some(unlink) = self.unlink.take() {
            unlink();
        }
    }
}
