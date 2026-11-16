use crate::types::*;
use address::Address;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::World;
use futures_util::FutureExt;
use lua_runtime_core::host::{self, HostDefinition, HostKind, Margatroid};
use lua_runtime_core::*;
use mcl_core::MclValue;
use mlua::{Lua, Value as MlValue};
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
        let storage = world.get_component::<AgentStorage>(request.owner).cloned();
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
    if let Some(binding) = world.get_resource::<tool_core::TaskSyncBinding>().cloned() {
        let address = source_address.clone();
        let tasks = tool_core::ToolTasks::new(request.owner, address, binding.execution_only());
        let (tx, _rx) = oneshot::channel();
        let (handle, input) = match tool_core::ToolTaskHandle::new(
            &tool_core::ToolExecutionLimits::default(),
            instance.start_id.clone(),
            request.program.origin.to_string_lossy().into_owned(),
            tx,
            tasks.update_context(),
            None,
        ) {
            Ok(value) => value,
            Err(error) => {
                reply_start(
                    &reply,
                    Err(failure(
                        LuaRuntimeErrorKind::EnvironmentFailed,
                        error.to_string(),
                    )),
                );
                return;
            }
        };
        if let Err(error) = tasks.register(instance.start_id.clone(), handle.clone()) {
            reply_start(
                &reply,
                Err(failure(
                    LuaRuntimeErrorKind::EnvironmentFailed,
                    error.to_string(),
                )),
            );
            return;
        }
        world.insert_component(request.owner, tasks.clone());
        world.insert_component(
            request.owner,
            tool_core::LuaExecutionTask {
                workspace: request.workspace,
                handle,
                tasks,
                input: Arc::new(tokio::sync::Mutex::new(input)),
                resource_dir: request.program.origin.clone(),
            },
        );
    }
    let wake = DriverWakeSlot::shared();
    let (ready_sender, ready) = oneshot::channel();
    let ready_slot = Arc::new(Mutex::new(Some(ready_sender)));
    let (ended_sender, ended) = oneshot::channel();
    let cancel = world
        .get_component::<tool_core::LuaExecutionTask>(request.owner)
        .map(|task| task.handle.cancel_token())
        .unwrap_or_default();
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
        workspace: request.workspace,
        services: world
            .get_resource::<lua_runtime_core::LuaServices>()
            .cloned(),
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
                if world
                    .remove_component::<tool_core::LuaExecutionTask>(instance.owner)
                    .is_some()
                {
                    world.remove_component::<tool_core::ToolTasks>(instance.owner);
                }
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
                if world
                    .remove_component::<tool_core::LuaExecutionTask>(output.instance.owner)
                    .is_some()
                {
                    world.remove_component::<tool_core::ToolTasks>(output.instance.owner);
                }
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
        services: context.services.clone(),
        identity: lua_runtime_core::LuaExecutionIdentity {
            source: context.instance.owner,
            workspace: context.workspace,
            address: context.source_address.clone(),
            instance: context.instance.start_id.clone(),
            cancel: context.cancel.clone(),
        },
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
    let mut failure_diagnostic = serde_json::Value::Null;
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
        failure_diagnostic = serde_json::json!({"message":brief,"raw_error":raw,"traceback":traceback,"origin":context.program.origin.to_string_lossy()});
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
    if let Some(services) = &context.services {
        let diagnostic = if failure_diagnostic.is_null() {
            result.as_ref().err().filter(|error|error.kind!=LuaRuntimeErrorKind::Cancelled).map(|error|serde_json::json!({"message":error.description,"raw_error":error.description,"origin":context.program.origin.to_string_lossy()})).unwrap_or(serde_json::Value::Null)
        } else {
            failure_diagnostic
        };
        if let Err(error) = services
            .call(
                lua_runtime_core::LuaExecutionIdentity {
                    source: context.instance.owner,
                    workspace: context.workspace,
                    address: context.source_address.clone(),
                    instance: context.instance.start_id.clone(),
                    cancel: context.cancel.clone(),
                },
                "execution_finished".into(),
                diagnostic,
            )
            .await
        {
            tracing::error!(%error,"Lua Agent monitor completion failed");
        }
    }
    let source = context.instance.owner;
    let id = context.instance.start_id.clone();
    let cleanup = context.closures.clone();
    let (tx, rx) = oneshot::channel();
    cleanup.send(move |world| {
        if let Some(task) = world
            .get_component::<tool_core::LuaExecutionTask>(source)
            .cloned()
        {
            task.handle.cancel();
            task.tasks.complete_cleanup(&id, &task.handle, None);
        }
        let _ = tx.send(());
    });
    let _ = rx.await;
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
        let _ = sender.send(result.clone());
    }
    let _ = ended.send(cleanup_result);
    events.send_event(DriverLuaVmExited { instance, result });
}

#[derive(Clone)]
struct DriverLogger {
    storage: Option<AgentStorage>,
    services: Option<lua_runtime_core::LuaServices>,
    identity: lua_runtime_core::LuaExecutionIdentity,
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
                    .and_then(|agent| agent.turn().clone())
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
        let Some(storage) = self.storage.clone() else {
            if record.raw_error.is_none()
                && let Some(services) = &self.services
            {
                let value = serde_json::to_value(&record).map_err(|error| {
                    failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string())
                })?;
                let count = services
                    .call(self.identity.clone(), "execution_log".into(), value)
                    .await?;
                if count.as_u64().is_some_and(|count| count > 0) {
                    return Ok(());
                }
            }
            let mut text = record.message;
            if let Some(raw) = record
                .raw_error
                .as_deref()
                .filter(|raw| !raw.is_empty() && *raw != text)
            {
                text.push(char::from(10));
                text.push_str(raw);
            }
            if let Some(traceback) = record
                .traceback
                .as_deref()
                .filter(|traceback| !traceback.is_empty() && !text.contains(*traceback))
            {
                text.push(char::from(10));
                text.push_str(traceback);
            }
            match record.level {
                AgentLogLevel::Debug => {
                    tracing::debug!(source=%self.source_address,origin=%self.origin,stage,"{text}")
                }
                AgentLogLevel::Info => {
                    tracing::info!(source=%self.source_address,origin=%self.origin,stage,"{text}")
                }
                AgentLogLevel::Warn => {
                    tracing::warn!(source=%self.source_address,origin=%self.origin,stage,"{text}")
                }
                AgentLogLevel::Error => {
                    tracing::error!(source=%self.source_address,origin=%self.origin,stage,"{text}")
                }
            }
            return Ok(());
        };
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

async fn execute_lua(
    context: &RunContext,
    mut environment: LuaEnvironment,
    logger: DriverLogger,
    processes: Arc<Mutex<Option<host::process::ProcessRegistry>>>,
) -> Result<(), LuaRuntimeError> {
    let program = &context.program;
    let root = context.root.clone();
    let cancel = context.cancel.clone();
    let config = &context.config;
    if cancel.is_cancelled() {
        return Err(failure(
            LuaRuntimeErrorKind::Cancelled,
            "Driver cancelled before execution",
        ));
    }
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
    let mut execution_context = match environment.globals.remove("execution_context") {
        Some(LuaBinding::Value(value)) => value.to_json()?,
        _ => serde_json::json!({"agent_info":agent_info,"project_root":root}),
    };
    if let Some(values) = execution_context.as_object_mut() {
        values.insert("project_root".into(), serde_json::json!(root));
        values.insert(
            "task_id".into(),
            serde_json::json!(context.instance.start_id),
        );
        values.insert(
            "resource_dir".into(),
            serde_json::json!(program.origin.parent().unwrap_or(std::path::Path::new("."))),
        );
    }
    let (lua, object) = lua_runtime_core::create_environment(
        HostDefinition {
            kind: HostKind::Driver,
            context: execution_context.clone(),
            arguments: serde_json::Value::Null,
        },
        config.max_memory_bytes,
    )
    .map_err(lua_error)?;
    let deadline_state = Arc::new(Mutex::new(None::<std::time::Instant>));
    let hook_deadline = deadline_state.clone();
    let checkpoint = cancel.clone();
    lua.set_global_hook(
        mlua::HookTriggers::new().every_nth_instruction(config.instruction_check_interval),
        move |_, _| {
            if checkpoint.is_cancelled()
                || lock(&hook_deadline)
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
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
    let saved = lua_runtime_core::configuration_globals(&lua).map_err(lua_error)?;
    lua_runtime_core::load_program(&lua, &program.source, &program.origin)
        .map_err(|error| logger.capture(error, LuaRuntimeErrorKind::CompileFailed))?;
    let configured: mlua::Value = lua
        .globals()
        .get::<mlua::Function>("configure")
        .map_err(lua_error)?
        .call((
            mlua::Value::Nil,
            host::data::to_lua(&lua, execution_context.clone()).map_err(lua_error)?,
        ))
        .map_err(lua_error)?;
    let configuration: tool_core::ToolConfiguration =
        serde_json::from_value(host::data::from_lua(&lua, configured).map_err(lua_error)?)
            .map_err(|e| failure(LuaRuntimeErrorKind::EnvironmentFailed, e.to_string()))?;
    configuration
        .validate()
        .map_err(|e| failure(LuaRuntimeErrorKind::EnvironmentFailed, e.to_string()))?;
    let deadline = configuration
        .timeout()
        .map(|duration| std::time::Instant::now() + duration);
    lua_runtime_core::enable_execution(&lua, saved).map_err(lua_error)?;
    *lock(&deadline_state) = deadline;
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
    let callbacks = lua.create_table().map_err(lua_error)?;
    if let Some(services) = context.services.clone() {
        let identity = lua_runtime_core::LuaExecutionIdentity {
            source: context.instance.owner,
            workspace: context.workspace,
            address: context.source_address.clone(),
            instance: context.instance.start_id.clone(),
            cancel: context.cancel.clone(),
        };
        for (callback, method) in [
            ("mcl_execute", "mcl_execute"),
            ("agent_create", "agent_create"),
            ("agent_stop", "agent_stop"),
            ("wait", "wait"),
            ("workspace_agents", "workspace_agents"),
            ("tools_register", "tools_register"),
            ("task_return_result", "task_return_result"),
            ("task_append_output", "task_append_output"),
            ("task_replace_output", "task_replace_output"),
            ("task_read_output", "task_read_output"),
            ("task_send", "task_send"),
            ("task_finish", "task_finish"),
            ("task_receive", "task_receive"),
        ] {
            let services = services.clone();
            let identity = identity.clone();
            let method = method.to_owned();
            callbacks.set(callback,lua.create_async_function(move|lua,args:mlua::MultiValue|{let services=services.clone();let identity=identity.clone();let method=method.clone();async move{
                let values=args.into_iter().map(|value|host::data::from_lua(&lua,value)).collect::<mlua::Result<Vec<_>>>()?;
                let arguments=if method=="mcl_execute"{serde_json::json!({"target":values.first(),"instruction":values.get(1),"arguments":values.get(2).filter(|value|!value.is_null()).cloned().unwrap_or_else(||serde_json::json!([]))})}else if method=="task_send"{serde_json::json!({"target":values.first(),"value":values.get(1)})}else{values.first().cloned().unwrap_or(serde_json::Value::Null)};
                let value=services.call(identity,method,arguments).await.map_err(mlua::Error::external)?;host::data::to_lua(&lua,value)
            }}).map_err(lua_error)?).map_err(lua_error)?;
        }
    }
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
        .globals()
        .get::<mlua::Function>("execute")
        .map_err(lua_error)?;
    let public_context = host::data::to_lua(&lua, execution_context).map_err(lua_error)?;
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(failure(LuaRuntimeErrorKind::Cancelled, "Lua execution cancelled")),
        () = async {if let Some(deadline)=deadline{tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;}else{std::future::pending::<()>().await;}}=>Err(failure(LuaRuntimeErrorKind::ExecutionFailed,"Lua execution deadline exceeded")),
        result = function.call_async::<()>((mlua::Value::Nil,public_context)) => {
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
