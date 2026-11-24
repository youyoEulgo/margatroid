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
    pub storage: Option<storage_core::AgentStorage>,
    pub execution_task: Option<tool_core::ToolTaskHandle>,
    pub workspace: core_plugin::Entity,
    pub services: Option<LuaServices>,
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
    pub reply: Option<StopDriverLuaVmReply>,
    pub thread: VmThread,
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
    pub reply: Option<StopDriverLuaVmReply>,
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

#[derive(Default)]
pub(crate) struct VmThreads {
    pub entries: std::collections::BTreeMap<String, VmThread>,
}
impl Resource for VmThreads {}

#[derive(Clone)]
pub(crate) struct VmThread {
    joined: tokio::sync::watch::Receiver<Option<Result<(), LuaRuntimeError>>>,
    pub start_settled: Arc<std::sync::atomic::AtomicBool>,
    pub exited: Arc<std::sync::atomic::AtomicBool>,
}
impl VmThread {
    pub fn spawn(
        context: RunContext,
        runtime: &async_runtime_plugin::AsyncRuntimeHandle,
    ) -> Result<Self, LuaRuntimeError> {
        let (finished, mut done) = tokio::sync::watch::channel(false);
        let (complete, joined) = tokio::sync::watch::channel(None);
        let join = std::thread::Builder::new()
            .name(format!("lua-{}", context.instance.start_id))
            .spawn(move || {
                let _finished = ThreadDone(finished);
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => tokio::task::LocalSet::new()
                        .block_on(&runtime, crate::handler::run_driver(context)),
                    Err(error) => {
                        let failure = LuaRuntimeError::new(
                            LuaRuntimeErrorKind::EnvironmentFailed,
                            error.to_string(),
                        );
                        if let Some(reply) = lock(&context.ready).take() {
                            let _ = reply.send(Err(failure.clone()));
                        }
                        let _ = context.ended.send(Err(failure.clone()));
                        context.events.send_event(DriverLuaVmExited {
                            instance: context.instance,
                            result: Err(failure),
                        });
                    }
                }
            })
            .map_err(|error| {
                LuaRuntimeError::new(LuaRuntimeErrorKind::EnvironmentFailed, error.to_string())
            })?;
        runtime.spawn(async move {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
            let result = tokio::task::spawn_blocking(move || join.join())
                .await
                .map_err(|error| {
                    LuaRuntimeError::new(LuaRuntimeErrorKind::CleanupFailed, error.to_string())
                })
                .and_then(|result| {
                    result.map_err(|_| {
                        LuaRuntimeError::new(
                            LuaRuntimeErrorKind::CleanupFailed,
                            "Lua VM thread panicked",
                        )
                    })
                });
            complete.send_replace(Some(result));
        });
        Ok(Self {
            joined,
            start_settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    pub async fn wait_finished(&self) -> Result<(), LuaRuntimeError> {
        let mut joined = self.joined.clone();
        loop {
            if let Some(result) = joined.borrow_and_update().clone() {
                return result;
            }
            joined.changed().await.map_err(|_| {
                LuaRuntimeError::new(
                    LuaRuntimeErrorKind::CleanupFailed,
                    "VM join supervision lost",
                )
            })?;
        }
    }
}
struct ThreadDone(tokio::sync::watch::Sender<bool>);
impl Drop for ThreadDone {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
