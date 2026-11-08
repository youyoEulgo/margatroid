use app_runtime_plugin::RuntimeEventSender;
use async_runtime_plugin::AsyncTaskError;
use core_plugin::Resource;
use lua_runtime_core::*;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) struct InstalledRuntime {
    pub schedule: String,
    pub config: LuaRuntimeConfig,
}
impl Resource for InstalledRuntime {}

pub(crate) struct RunContext {
    pub storage: storage_core::AgentStorage,
    pub closures: closure_plugin::WorldClosureSender,
    pub root: std::path::PathBuf,
    pub source_address: String,
    pub instance: DriverLuaVmInstance,
    pub program: LuaProgram,
    pub environment: LuaEnvironment,
    pub wake: Arc<Mutex<DriverWakeSlot>>,
    pub ready: DriverReadySender,
    pub ended: oneshot::Sender<Result<(), LuaRuntimeError>>,
    pub cancel: CancellationToken,
    pub events: RuntimeEventSender,
    pub config: LuaRuntimeConfig,
}

pub(crate) struct StartContext {
    pub instance: DriverLuaVmInstance,
    pub ready: oneshot::Receiver<Result<(), LuaRuntimeError>>,
    pub cancel: CancellationToken,
    pub events: RuntimeEventSender,
    pub reply: StartDriverLuaVmReply,
    pub config: LuaRuntimeConfig,
}

pub(crate) struct StopContext {
    pub instance: DriverLuaVmInstance,
    pub ended: oneshot::Receiver<Result<(), LuaRuntimeError>>,
    pub reply: StopDriverLuaVmReply,
}

#[derive(Clone, Debug)]
pub(crate) struct StartOutput {
    pub instance: DriverLuaVmInstance,
    pub reply: StartDriverLuaVmReply,
}

#[derive(Clone, Debug)]
pub(crate) struct StartError {
    pub instance: Option<DriverLuaVmInstance>,
    pub reply: Option<StartDriverLuaVmReply>,
    pub error: LuaRuntimeError,
    pub cleanup_completed: bool,
}

impl From<AsyncTaskError> for StartError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            instance: None,
            reply: None,
            error: LuaRuntimeError::new(LuaRuntimeErrorKind::CleanupFailed, error.to_string()),
            cleanup_completed: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StopOutput {
    pub instance: DriverLuaVmInstance,
    pub reply: StopDriverLuaVmReply,
}

#[derive(Clone, Debug)]
pub(crate) struct StopError {
    pub instance: Option<DriverLuaVmInstance>,
    pub reply: Option<StopDriverLuaVmReply>,
    pub error: LuaRuntimeError,
}

impl From<AsyncTaskError> for StopError {
    fn from(error: AsyncTaskError) -> Self {
        Self {
            instance: None,
            reply: None,
            error: LuaRuntimeError::new(LuaRuntimeErrorKind::CleanupFailed, error.to_string()),
        }
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
