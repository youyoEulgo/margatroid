use crate::types::*;
use address::Address;
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::World;
use futures_util::FutureExt;
use lua_runtime_core::host::{self, HostDefinition, HostKind, Margatroid};
use lua_runtime_core::*;
use mcl_core::{MclCommand, MclInstruction, MclValue};
use mlua::{Lua, LuaOptions, StdLib, Value as MlValue};
use std::collections::{BTreeMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use storage_core::{AgentLogLevel, AgentLogRecord, AgentLogSource, AgentStorage, append_agent_log};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::WorkspaceExecutor;

fn failure(kind: LuaRuntimeErrorKind, description: impl Into<String>) -> LuaRuntimeError {
    LuaRuntimeError::new(kind, description)
}

fn next_id(prefix: &str) -> String {
    static PROCESS: OnceLock<String> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let process = PROCESS.get_or_init(|| {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock must follow Unix epoch")
                .as_nanos()
        )
    });
    format!(
        "{prefix}-{process}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn handle_start_driver_lua_vm(world: &mut World, request: StartDriverLuaVm) {
    let Some(sender) = lock(&request.reply).take() else {
        return;
    };
    let reply = Arc::new(Mutex::new(Some(sender)));
    let settings = world
        .get_resource::<InstalledRuntime>()
        .expect("Lua runtime is not installed");
    let config = settings.config.clone();
    let schedule = settings.schedule.clone();
    let entry = (|| {
        config.validate()?;
        if !world.is_alive(request.owner) || !world.is_alive(request.workspace) {
            return Err(failure(
                LuaRuntimeErrorKind::InstanceNotFound,
                "VM owner or Workspace is missing",
            ));
        }
        if world.get_component::<DriverLuaVm>(request.owner).is_some() {
            return Err(failure(
                LuaRuntimeErrorKind::InvalidRequest,
                "VM already exists on owner",
            ));
        }
        if request.program.source.len() > config.max_source_bytes {
            return Err(failure(
                LuaRuntimeErrorKind::SourceTooLarge,
                "Lua source exceeds limit",
            ));
        }
        validate_environment(&request.environment)?;
        let source_address = world
            .get_component::<Address>(request.owner)
            .ok_or_else(|| {
                failure(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "VM owner address is missing",
                )
            })?
            .as_str()
            .to_owned();
        let entry = world
            .get_component::<WorkspaceExecutor>(request.workspace)
            .map(|executor| executor.entry.clone())
            .ok_or_else(|| {
                failure(
                    LuaRuntimeErrorKind::InstanceNotFound,
                    "Workspace executor is missing",
                )
            })?;
        let storage = world
            .get_component::<AgentStorage>(request.owner)
            .cloned()
            .ok_or_else(|| {
                failure(
                    LuaRuntimeErrorKind::EnvironmentFailed,
                    "AgentStorage is missing",
                )
            })?;
        let root = world
            .get_component::<workspace_core::Workspace>(request.workspace)
            .map(|workspace| workspace.project_root.clone())
            .ok_or_else(|| {
                failure(
                    LuaRuntimeErrorKind::EnvironmentFailed,
                    "Workspace is missing",
                )
            })?;
        Ok((entry, source_address, storage, root))
    })();
    let (entry, source_address, storage, root) = match entry {
        Ok(entry) => entry,
        Err(error) => {
            reply_start(&reply, Err(error));
            return;
        }
    };
    let instance = DriverLuaVmInstance {
        owner: request.owner,
        start_id: next_id("driver"),
    };
    let wake = DriverWakeSlot::shared();
    let (ready_sender, ready) = oneshot::channel();
    let ready_slot = Arc::new(Mutex::new(Some(ready_sender)));
    let (ended_sender, ended) = oneshot::channel();
    let cancel = CancellationToken::new();
    world.insert_component(
        request.owner,
        DriverLuaVm {
            instance: instance.clone(),
            wake: wake.clone(),
            ready: ready_slot.clone(),
            ended: Some(ended),
            cancel: cancel.clone(),
        },
    );
    let events = world.event_sender();
    let run = RunContext {
        storage,
        closures: world.closure_sender(&schedule),
        root,
        source_address,
        instance: instance.clone(),
        program: request.program,
        environment: request.environment,
        wake,
        ready: ready_slot,
        ended: ended_sender,
        cancel: cancel.clone(),
        events: events.clone(),
        config: config.clone(),
    };
    if let Err(error) = entry.submit(Box::new(move || -> workspace_core::LocalFuture {
        Box::pin(run_driver(run))
    })) {
        cancel.cancel();
        world.remove_component::<DriverLuaVm>(instance.owner);
        reply_start(
            &reply,
            Err(failure(LuaRuntimeErrorKind::Closed, error.to_string())),
        );
        return;
    }
    let context = StartContext {
        instance,
        ready,
        cancel,
        events,
        reply,
        config,
    };
    world.send_async_closure(&schedule, move || start_vm(context));
}

pub(crate) fn handle_start_result(world: &mut World, result: Result<StartOutput, StartError>) {
    match result {
        Ok(output) => {
            let result = if current_instance(world, &output.instance) {
                Ok(output.instance)
            } else {
                Err(failure(
                    LuaRuntimeErrorKind::InstanceMismatch,
                    "VM changed while waiting for ready",
                ))
            };
            reply_start(&output.reply, result);
        }
        Err(error) => {
            if let Some(instance) = &error.instance
                && error.cleanup_completed
                && current_instance(world, instance)
            {
                world.remove_component::<DriverLuaVm>(instance.owner);
            }
            if let Some(reply) = error.reply {
                reply_start(&reply, Err(error.error));
            } else {
                tracing::error!(error = %error.error, "Lua start coordination failed");
            }
        }
    }
}

pub(crate) fn handle_stop_driver_lua_vm(world: &mut World, request: StopDriverLuaVm) {
    let Some(sender) = lock(&request.reply).take() else {
        return;
    };
    let reply = Arc::new(Mutex::new(Some(sender)));
    let ended = (|| {
        let vm = world
            .get_component_mut::<DriverLuaVm>(request.instance.owner)
            .ok_or_else(|| failure(LuaRuntimeErrorKind::InstanceNotFound, "VM is missing"))?;
        if vm.instance != request.instance {
            return Err(failure(
                LuaRuntimeErrorKind::InstanceMismatch,
                "VM stop instance does not match",
            ));
        }
        let ended = vm.ended.take().ok_or_else(|| {
            failure(
                LuaRuntimeErrorKind::StopAlreadyWaiting,
                "VM ended receiver was already taken",
            )
        })?;
        vm.cancel.cancel();
        lock(&vm.wake).close();
        Ok(ended)
    })();
    let ended = match ended {
        Ok(ended) => ended,
        Err(error) => {
            reply_stop(&reply, Err(error));
            return;
        }
    };
    let schedule = world
        .get_resource::<InstalledRuntime>()
        .unwrap()
        .schedule
        .clone();
    let context = StopContext {
        instance: request.instance,
        ended,
        reply,
    };
    world.send_async_closure(&schedule, move || stop_vm(context));
}

pub(crate) fn handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>) {
    match result {
        Ok(output) => {
            let result = if current_instance(world, &output.instance) {
                world.remove_component::<DriverLuaVm>(output.instance.owner);
                Ok(())
            } else {
                Err(failure(
                    LuaRuntimeErrorKind::InstanceMismatch,
                    "VM changed while waiting for stop",
                ))
            };
            reply_stop(&output.reply, result);
        }
        Err(error) => {
            if let Some(instance) = error.instance {
                tracing::error!(?instance, error = %error.error, "Lua VM stop failed");
            }
            if let Some(reply) = error.reply {
                reply_stop(&reply, Err(error.error));
            } else {
                tracing::error!(error = %error.error, "Lua stop coordination failed");
            }
        }
    }
}

pub(crate) fn handle_driver_lua_vm_exited(world: &mut World, event: DriverLuaVmExited) {
    if current_instance(world, &event.instance)
        && let Err(error) = event.result
        && error.kind != LuaRuntimeErrorKind::Cancelled
    {
        tracing::error!(instance = ?event.instance, %error, "Driver Lua VM exited with an error");
    }
}

fn current_instance(world: &World, instance: &DriverLuaVmInstance) -> bool {
    world
        .get_component::<DriverLuaVm>(instance.owner)
        .is_some_and(|vm| vm.instance == *instance)
}

fn reply_start(
    reply: &StartDriverLuaVmReply,
    result: Result<DriverLuaVmInstance, LuaRuntimeError>,
) {
    if let Some(sender) = lock(reply).take() {
        let _ = sender.send(result);
    }
}

fn reply_stop(reply: &StopDriverLuaVmReply, result: Result<(), LuaRuntimeError>) {
    if let Some(sender) = lock(reply).take() {
        let _ = sender.send(result);
    }
}

async fn start_vm(context: StartContext) -> Result<StartOutput, StartError> {
    let StartContext {
        instance,
        ready,
        cancel,
        events,
        reply,
        config,
    } = context;
    let readiness = if let Some(timeout) = config.startup_timeout {
        tokio::time::timeout(timeout, ready)
            .await
            .map_err(|_| {
                failure(
                    LuaRuntimeErrorKind::StartupTimeout,
                    "Driver ready timed out",
                )
            })
            .and_then(|result| {
                result.map_err(|_| {
                    failure(
                        LuaRuntimeErrorKind::CleanupFailed,
                        "Driver ready sender was lost",
                    )
                })
            })
            .and_then(|result| result)
    } else {
        ready.await.unwrap_or_else(|_| {
            Err(failure(
                LuaRuntimeErrorKind::CleanupFailed,
                "Driver ready sender was lost",
            ))
        })
    };
    match readiness {
        Ok(()) => Ok(StartOutput { instance, reply }),
        Err(error) => {
            cancel.cancel();
            let (sender, receiver) = oneshot::channel();
            events.send_event(StopDriverLuaVm {
                instance: instance.clone(),
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            let cleanup = receiver.await.unwrap_or_else(|_| {
                Err(failure(
                    LuaRuntimeErrorKind::CleanupFailed,
                    "VM startup cleanup reply was lost",
                ))
            });
            let cleanup_completed = cleanup.is_ok();
            let error = match cleanup {
                Ok(()) => error,
                Err(cleanup) => failure(
                    LuaRuntimeErrorKind::CleanupFailed,
                    format!("{error}; startup cleanup failed: {cleanup}"),
                ),
            };
            Err(StartError {
                instance: Some(instance),
                reply: Some(reply),
                error,
                cleanup_completed,
            })
        }
    }
}

async fn stop_vm(context: StopContext) -> Result<StopOutput, StopError> {
    let StopContext {
        instance,
        ended,
        reply,
    } = context;
    match ended.await.unwrap_or_else(|_| {
        Err(failure(
            LuaRuntimeErrorKind::EndNotificationLost,
            "VM ended sender was lost",
        ))
    }) {
        Ok(()) => Ok(StopOutput { instance, reply }),
        Err(error) => Err(StopError {
            instance: Some(instance),
            reply: Some(reply),
            error,
        }),
    }
}

async fn run_driver(mut context: RunContext) {
    let logger = DriverLogger {
        storage: context.storage.clone(),
        closures: context.closures.clone(),
        instance: context.instance.clone(),
        source_address: context.source_address.clone(),
        origin: context.program.origin.to_string_lossy().into_owned(),
        diagnostic: Arc::new(Mutex::new(None)),
    };
    let processes = Arc::new(Mutex::new(None::<host::process::ProcessRegistry>));
    let environment = std::mem::take(&mut context.environment);
    let mut result = AssertUnwindSafe(execute_lua(
        &context,
        environment,
        logger.clone(),
        processes.clone(),
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err(failure(
            LuaRuntimeErrorKind::ExecutionFailed,
            "Driver Lua task panicked",
        ))
    });
    if let Err(error) = &result
        && error.kind != LuaRuntimeErrorKind::Cancelled
    {
        let raw = error.description.clone();
        let diagnostic = lock(&logger.diagnostic).take();
        let brief = diagnostic
            .as_ref()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| raw.lines().next().unwrap_or("Lua failure").to_owned());
        let traceback = diagnostic.and_then(|d| d.traceback);
        let stage = match error.kind {
            LuaRuntimeErrorKind::CompileFailed => "compile",
            LuaRuntimeErrorKind::CleanupFailed => "cleanup",
            _ => "execute",
        };
        if let Err(log_error) = logger
            .append(
                AgentLogLevel::Error,
                stage,
                brief.clone(),
                None,
                Some(raw),
                traceback,
            )
            .await
        {
            result = Err(failure(
                LuaRuntimeErrorKind::ExecutionFailed,
                format!("{error}; log persistence failed: {log_error}"),
            ));
        } else {
            result = Err(failure(error.kind, brief));
        }
    }
    let mut cleanup_result = Ok(());
    let registry = lock(&processes).take();
    if let Some(registry) = registry
        && let Err(error) = registry.cleanup().await
    {
        let raw = error.to_string();
        let mut error = failure(LuaRuntimeErrorKind::CleanupFailed, raw.clone());
        if let Err(log_error) = logger
            .append(
                AgentLogLevel::Error,
                "cleanup",
                raw.clone(),
                None,
                Some(raw),
                None,
            )
            .await
        {
            error
                .description
                .push_str(&format!("; log persistence failed: {log_error}"));
        }
        cleanup_result = Err(error.clone());
        result = Err(error);
    }
    let RunContext {
        instance,
        wake,
        ready,
        ended,
        events,
        ..
    } = context;
    lock(&wake).close();
    if let Some(sender) = lock(&ready).take() {
        let error = result.as_ref().err().cloned().unwrap_or_else(|| {
            failure(
                LuaRuntimeErrorKind::ExecutionFailed,
                "Driver exited before ready",
            )
        });
        let _ = sender.send(Err(error));
    }
    let _ = ended.send(cleanup_result);
    events.send_event(DriverLuaVmExited { instance, result });
}

#[derive(Clone)]
struct DriverLogger {
    storage: AgentStorage,
    closures: WorldClosureSender,
    instance: DriverLuaVmInstance,
    source_address: String,
    origin: String,
    diagnostic: Arc<Mutex<Option<host::LuaFailureDiagnostic>>>,
}
impl DriverLogger {
    fn capture(&self, error: mlua::Error, kind: LuaRuntimeErrorKind) -> LuaRuntimeError {
        *lock(&self.diagnostic) = Some(host::LuaFailureDiagnostic::capture_with(&error, |cause| {
            cause
                .downcast_ref::<LuaRuntimeError>()
                .map(|error| error.description.clone())
        }));
        failure(kind, error.to_string())
    }
    async fn append(
        &self,
        level: AgentLogLevel,
        stage: &str,
        message: String,
        details_json: Option<serde_json::Value>,
        raw_error: Option<String>,
        traceback: Option<String>,
    ) -> Result<(), LuaRuntimeError> {
        let (sender, receiver) = oneshot::channel();
        let instance = self.instance.clone();
        self.closures.send(move |world| {
            let turn = if current_instance(world, &instance) {
                world
                    .get_component::<agent_core::Agent>(instance.owner)
                    .and_then(|agent| agent.turn.clone())
            } else {
                None
            };
            let _ = sender.send(turn);
        });
        let turn = receiver
            .await
            .map_err(|_| failure(LuaRuntimeErrorKind::Closed, "log context unavailable"))?;
        let record = AgentLogRecord {
            created_at_ms: None,
            level,
            source: AgentLogSource::Driver,
            stage: stage.into(),
            message,
            details_json,
            context_json: Some(
                serde_json::json!({"instance": self.instance.start_id, "source_address": self.source_address, "turn": turn}),
            ),
            origin: Some(self.origin.clone()),
            raw_error,
            traceback,
        };
        let storage = self.storage.clone();
        tokio::task::spawn_blocking(move || append_agent_log(&storage, record))
            .await
            .map_err(|error| failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string()))?
            .map_err(|error| failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string()))?;
        Ok(())
    }
}

fn validate_environment(environment: &LuaEnvironment) -> Result<(), LuaRuntimeError> {
    for name in environment.globals.keys() {
        if name.is_empty()
            || name.chars().any(char::is_control)
            || matches!(name.as_str(), "mcl" | "wait" | "require" | "margatroid")
        {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "invalid or reserved global binding",
            ));
        }
    }
    for name in environment.modules.keys() {
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "invalid module name",
            ));
        }
    }
    Ok(())
}

struct WaitHost {
    slot: Arc<Mutex<DriverWakeSlot>>,
}
impl LuaHostFunction for WaitHost {
    fn call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture {
        let slot = self.slot.clone();
        Box::pin(async move {
            if !arguments.is_empty() {
                return Err(failure(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "wait expects no arguments",
                ));
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => Err(failure(LuaRuntimeErrorKind::Cancelled, "Driver wait cancelled")),
                result = DriverWakeSlot::wait(slot) => result.map(|()| LuaValue::Nil),
            }
        })
    }
}

struct MclHost {
    source_address: String,
    instance: DriverLuaVmInstance,
    events: RuntimeEventSender,
    cancel: CancellationToken,
}
impl LuaHostFunction for MclHost {
    fn call(&self, arguments: Vec<LuaValue>, cancel: CancellationToken) -> HostFuture {
        let instance = self.instance.clone();
        let source_address = self.source_address.clone();
        let events = self.events.clone();
        let vm_cancel = self.cancel.clone();
        Box::pin(async move {
            let request_id = next_id("lua-mcl");
            let (text, parameters) = match mcl_arguments(&instance, arguments) {
                Ok(arguments) => arguments,
                Err(error) => return mcl_error_value(&request_id, error),
            };
            if vm_cancel.is_cancelled() || cancel.is_cancelled() {
                return Err(failure(
                    LuaRuntimeErrorKind::Cancelled,
                    "MCL host cancelled",
                ));
            }
            let command_cancel = vm_cancel.child_token();
            let (sender, receiver) = oneshot::channel();
            events.send_event(MclCommand {
                id: request_id.clone(),
                source: instance.owner,
                source_address,
                source_instance: Some(instance.start_id),
                target: instance.owner,
                instruction: MclInstruction {
                    text,
                    arguments: parameters,
                },
                cancel: command_cancel.clone(),
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            let value = tokio::select! {
                biased;
                () = vm_cancel.cancelled() => { command_cancel.cancel(); return Err(failure(LuaRuntimeErrorKind::Cancelled, "MCL host cancelled")); },
                () = cancel.cancelled() => { command_cancel.cancel(); return Err(failure(LuaRuntimeErrorKind::Cancelled, "MCL call cancelled")); },
                result = receiver => match result {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return mcl_error_value(&request_id, error),
                    Err(_) => return mcl_error_value(&request_id, failure(LuaRuntimeErrorKind::Closed, "MCL reply was lost")),
                },
            };
            if matches!(value, MclValue::Unit) {
                return Ok(LuaValue::Nil);
            }
            let value = serde_json::to_value(value).map_err(|error| {
                failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string())
            })?;
            LuaValue::from_json(value)
        })
    }
}

fn mcl_error_value(
    request_id: &str,
    error: impl std::fmt::Display,
) -> Result<LuaValue, LuaRuntimeError> {
    let value = MclValue::Error(serde_json::json!({
        "type": "error", "message": error.to_string(), "request_id": request_id,
    }));
    LuaValue::from_json(
        serde_json::to_value(value)
            .map_err(|error| failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string()))?,
    )
}

fn mcl_arguments(
    instance: &DriverLuaVmInstance,
    arguments: Vec<LuaValue>,
) -> Result<(String, Vec<serde_json::Value>), LuaRuntimeError> {
    let invalid = || {
        failure(
            LuaRuntimeErrorKind::InvalidRequest,
            "mcl expects an optional matching owner entity, instruction text and optional argument array",
        )
    };
    let mut arguments = arguments.into_iter();
    let first = arguments.next().ok_or_else(invalid)?;
    let text = match first {
        LuaValue::String(text) => text,
        LuaValue::Object(entity) => {
            if entity.get("index") != Some(&LuaValue::Integer(i64::from(instance.owner.index())))
                || entity.get("generation")
                    != Some(&LuaValue::Integer(i64::from(instance.owner.generation())))
            {
                return Err(invalid());
            }
            match arguments.next() {
                Some(LuaValue::String(text)) => text,
                _ => return Err(invalid()),
            }
        }
        _ => return Err(invalid()),
    };
    if text.trim().is_empty() {
        return Err(invalid());
    }
    let parameters = match arguments.next() {
        None | Some(LuaValue::Nil) => Vec::new(),
        Some(LuaValue::Array(values)) => values
            .into_iter()
            .map(|value| value.to_json())
            .collect::<Result<_, _>>()?,
        Some(LuaValue::Object(values)) if values.is_empty() => Vec::new(),
        _ => return Err(invalid()),
    };
    if arguments.next().is_some() {
        return Err(invalid());
    }
    Ok((text, parameters))
}

async fn execute_lua(
    context: &RunContext,
    mut environment: LuaEnvironment,
    logger: DriverLogger,
    processes: Arc<Mutex<Option<host::process::ProcessRegistry>>>,
) -> Result<(), LuaRuntimeError> {
    let program = &context.program;
    let root = context.root.clone();
    let mcl_host = MclHost {
        instance: context.instance.clone(),
        source_address: context.source_address.clone(),
        events: context.events.clone(),
        cancel: context.cancel.clone(),
    };
    let wake = context.wake.clone();
    let cancel = context.cancel.clone();
    let config = &context.config;
    if cancel.is_cancelled() {
        return Err(failure(
            LuaRuntimeErrorKind::Cancelled,
            "Driver cancelled before execution",
        ));
    }
    let lua = match program.libraries {
        LuaStandardLibraries::Minimal => {
            Lua::new_with(StdLib::NONE, LuaOptions::default()).map_err(lua_error)?
        }
        LuaStandardLibraries::Safe => {
            Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).map_err(lua_error)?
        }
        LuaStandardLibraries::Full => unsafe {
            Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default())
        },
    };
    lua.set_memory_limit(config.max_memory_bytes)
        .map_err(lua_error)?;
    if program.libraries == LuaStandardLibraries::Minimal {
        for name in [
            "dofile",
            "loadfile",
            "load",
            "loadstring",
            "collectgarbage",
            "print",
            "warn",
        ] {
            lua.globals().set(name, MlValue::Nil).map_err(lua_error)?;
        }
    }
    let checkpoint = cancel.clone();
    lua.set_global_hook(
        mlua::HookTriggers::new().every_nth_instruction(config.instruction_check_interval),
        move |_, _| {
            if checkpoint.is_cancelled() {
                Err(mlua::Error::external(failure(
                    LuaRuntimeErrorKind::Cancelled,
                    "Driver cancelled",
                )))
            } else {
                Ok(mlua::VmState::Continue)
            }
        },
    )
    .map_err(lua_error)?;
    let agent_info = match environment.globals.remove("agent_info") {
        Some(LuaBinding::Value(value)) => value.to_json()?,
        None => serde_json::Value::Null,
        Some(LuaBinding::Function(_)) => {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "agent_info must be data",
            ));
        }
    };
    let object = Margatroid::install(&lua, HostDefinition {
        kind: HostKind::Driver,
        context: serde_json::json!({"agent_info": agent_info, "instance": logger.instance.start_id, "origin": logger.origin, "project_root": root}),
        arguments: serde_json::Value::Null,
    }).map_err(lua_error)?;
    for (name, binding) in environment.globals {
        lua.globals()
            .set(name, binding_value(&lua, binding, &cancel)?)
            .map_err(lua_error)?;
    }
    let modules = lua.create_table().map_err(lua_error)?;
    for (name, binding) in environment.modules {
        modules
            .raw_set(name, binding_value(&lua, binding, &cancel)?)
            .map_err(lua_error)?;
    }
    lua.globals()
        .set(
            "require",
            lua.create_function(move |_, name: String| {
                let value = modules.raw_get::<MlValue>(name.as_str())?;
                if matches!(value, MlValue::Nil) {
                    Err(mlua::Error::RuntimeError(format!(
                        "module {name} is not injected"
                    )))
                } else {
                    Ok(value)
                }
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    let mcl: Arc<dyn LuaHostFunction> = Arc::new(mcl_host);
    let wait: Arc<dyn LuaHostFunction> = Arc::new(WaitHost { slot: wake });
    let callbacks = lua.create_table().map_err(lua_error)?;
    callbacks
        .set("driver_mcl", host_function_value(&lua, mcl, &cancel, true)?)
        .map_err(lua_error)?;
    callbacks
        .set(
            "driver_wait",
            host_function_value(&lua, wait, &cancel, false)?,
        )
        .map_err(lua_error)?;
    host::data::install(&lua, &callbacks, root.clone()).map_err(lua_error)?;
    let (registry, constructor) = host::process::install_process(&lua, root).map_err(lua_error)?;
    *lock(&processes) = Some(registry);
    callbacks
        .set("process_start", constructor)
        .map_err(lua_error)?;
    callbacks
        .set("wait_any", host::install_wait_any(&lua).map_err(lua_error)?)
        .map_err(lua_error)?;
    let log_callback = logger.clone();
    callbacks
        .set(
            "log",
            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let logger = log_callback.clone();
                async move {
                    if !(2..=3).contains(&args.len()) {
                        return Err(mlua::Error::external(failure(
                            LuaRuntimeErrorKind::InvalidRequest,
                            "log expects level, message and optional details",
                        )));
                    }
                    let mut args = args.into_iter();
                    let level = match args.next() {
                        Some(MlValue::String(level)) => match level.to_str()?.as_ref() {
                            "debug" => AgentLogLevel::Debug,
                            "info" => AgentLogLevel::Info,
                            "warn" => AgentLogLevel::Warn,
                            "error" => AgentLogLevel::Error,
                            _ => {
                                return Err(mlua::Error::external(failure(
                                    LuaRuntimeErrorKind::InvalidRequest,
                                    "invalid log level",
                                )));
                            }
                        },
                        _ => {
                            return Err(mlua::Error::external(failure(
                                LuaRuntimeErrorKind::InvalidRequest,
                                "log level must be a string",
                            )));
                        }
                    };
                    let message = match args.next() {
                        Some(MlValue::String(message)) => message.to_str()?.to_owned(),
                        _ => {
                            return Err(mlua::Error::external(failure(
                                LuaRuntimeErrorKind::InvalidRequest,
                                "log message must be a string",
                            )));
                        }
                    };
                    let details = match args.next() {
                        None | Some(MlValue::Nil) => None,
                        Some(value) => Some(host::data::from_lua_object(&lua, value)?),
                    };
                    logger
                        .append(level, "script", message, details, None, None)
                        .await
                        .map_err(mlua::Error::external)
                }
            })
            .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    Margatroid::enable(&object, callbacks).map_err(lua_error)?;
    let function = lua
        .load(&program.source)
        .set_name(program.origin.to_string_lossy())
        .into_function()
        .map_err(|error| logger.capture(error, LuaRuntimeErrorKind::CompileFailed))?;
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(failure(LuaRuntimeErrorKind::Cancelled, "Driver cancelled")),
        result = function.call_async::<()>(()) => {
            if cancel.is_cancelled() { Err(failure(LuaRuntimeErrorKind::Cancelled, "Driver cancelled")) }
            else { result.map_err(|error| logger.capture(error, LuaRuntimeErrorKind::ExecutionFailed)) }
        }
    }
}

fn lua_error(error: mlua::Error) -> LuaRuntimeError {
    failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string())
}

fn binding_value(
    lua: &Lua,
    binding: LuaBinding,
    cancel: &CancellationToken,
) -> Result<MlValue, LuaRuntimeError> {
    match binding {
        LuaBinding::Value(value) => to_ml_value(lua, value),
        LuaBinding::Function(function) => host_function_value(lua, function, cancel, false),
    }
}

fn host_function_value(
    lua: &Lua,
    function: Arc<dyn LuaHostFunction>,
    cancel: &CancellationToken,
    error_values: bool,
) -> Result<MlValue, LuaRuntimeError> {
    let cancel = cancel.clone();
    let callback = lua.create_async_function(move |lua, arguments: mlua::MultiValue| {
        let function = function.clone();
        let cancel = cancel.clone();
        async move {
            let mut seen = HashSet::new();
            let arguments = arguments.into_iter().map(|value| from_ml_value(value, &mut seen, 0)).collect::<Result<_, _>>();
            let result = match arguments {
                Ok(arguments) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(failure(LuaRuntimeErrorKind::Cancelled, "Lua host cancelled")),
                    result = function.call(arguments, cancel.clone()) => result,
                },
                Err(error) => Err(error),
            };
            let value = match result {
                Ok(value) => value,
                Err(error) if error_values && error.kind != LuaRuntimeErrorKind::Cancelled => {
                    mcl_error_value(&next_id("lua-mcl"), error).map_err(mlua::Error::external)?
                }
                Err(error) => return Err(mlua::Error::external(error)),
            };
            to_ml_value(&lua, value).map_err(mlua::Error::external)
        }
    }).map_err(lua_error)?;
    Ok(MlValue::Function(callback))
}

fn to_ml_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError> {
    Ok(match value {
        LuaValue::Nil => MlValue::Nil,
        LuaValue::Boolean(value) => MlValue::Boolean(value),
        LuaValue::Integer(value) => MlValue::Integer(value),
        LuaValue::Number(value) if value.is_finite() => MlValue::Number(value),
        LuaValue::Number(_) => {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "non-finite Lua host number",
            ));
        }
        LuaValue::String(value) => MlValue::String(lua.create_string(value).map_err(lua_error)?),
        LuaValue::Array(values) => {
            let table = lua.create_table().map_err(lua_error)?;
            let metatable = lua.create_table().map_err(lua_error)?;
            metatable
                .raw_set("__margatroid_array", true)
                .map_err(lua_error)?;
            metatable
                .raw_set("__metatable", "array")
                .map_err(lua_error)?;
            table.set_metatable(Some(metatable)).map_err(lua_error)?;
            for (index, value) in values.into_iter().enumerate() {
                table
                    .raw_set(index + 1, stored_value(lua, value)?)
                    .map_err(lua_error)?;
            }
            MlValue::Table(table)
        }
        LuaValue::Object(values) => {
            let table = lua.create_table().map_err(lua_error)?;
            for (key, value) in values {
                table
                    .raw_set(key, stored_value(lua, value)?)
                    .map_err(lua_error)?;
            }
            MlValue::Table(table)
        }
    })
}

fn stored_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError> {
    if matches!(value, LuaValue::Nil) {
        Ok(MlValue::LightUserData(mlua::LightUserData(
            std::ptr::null_mut(),
        )))
    } else {
        to_ml_value(lua, value)
    }
}

fn from_ml_value(
    value: MlValue,
    seen: &mut HashSet<usize>,
    depth: usize,
) -> Result<LuaValue, LuaRuntimeError> {
    if depth > 128 {
        return Err(failure(
            LuaRuntimeErrorKind::ExecutionFailed,
            "Lua host value is nested too deeply",
        ));
    }
    Ok(match value {
        MlValue::Nil => LuaValue::Nil,
        MlValue::Boolean(value) => LuaValue::Boolean(value),
        MlValue::Integer(value) => LuaValue::Integer(value),
        MlValue::Number(value) if value.is_finite() => LuaValue::Number(value),
        MlValue::String(value) => LuaValue::String(value.to_str().map_err(lua_error)?.to_owned()),
        MlValue::LightUserData(value) if value.0.is_null() => LuaValue::Nil,
        MlValue::Table(table) => {
            let identity = table.to_pointer() as usize;
            if !seen.insert(identity) {
                return Err(failure(
                    LuaRuntimeErrorKind::ExecutionFailed,
                    "cyclic Lua table cannot be a host value",
                ));
            }
            let tagged = table
                .metatable()
                .map(|meta| meta.raw_get::<bool>("__margatroid_array").unwrap_or(false))
                .unwrap_or(false);
            let mut integers = BTreeMap::new();
            let mut object = BTreeMap::new();
            for pair in table.pairs::<MlValue, MlValue>() {
                let (key, value) = pair.map_err(lua_error)?;
                let value = from_ml_value(value, seen, depth + 1)?;
                match key {
                    MlValue::Integer(index) if index > 0 => {
                        integers.insert(index, value);
                    }
                    MlValue::String(key) => {
                        object.insert(key.to_str().map_err(lua_error)?.to_owned(), value);
                    }
                    _ => {
                        return Err(failure(
                            LuaRuntimeErrorKind::ExecutionFailed,
                            "unsupported Lua table key",
                        ));
                    }
                }
            }
            seen.remove(&identity);
            if !integers.is_empty() || tagged {
                if !object.is_empty() || integers.keys().copied().ne(1..=integers.len() as i64) {
                    return Err(failure(
                        LuaRuntimeErrorKind::ExecutionFailed,
                        "Lua table is not a contiguous array",
                    ));
                }
                LuaValue::Array(integers.into_values().collect())
            } else {
                LuaValue::Object(object)
            }
        }
        _ => {
            return Err(failure(
                LuaRuntimeErrorKind::ExecutionFailed,
                "Lua value cannot cross the host boundary",
            ));
        }
    })
}
