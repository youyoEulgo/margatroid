use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use crate::types::{LuaRuntimeConfig, LuaRuntimeSchedule};
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use futures_util::FutureExt;
use margatroid_types::agent_components::{Agent, AgentState};
use margatroid_types::execution_components::{AgentExecutionScope, AgentMclDrainHandle};
use margatroid_types::lua_environment::*;
use margatroid_types::lua_runtime_components::*;
use margatroid_types::lua_runtime_events::*;
use mlua::{Lua, LuaOptions, StdLib, Value as MlValue};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn failure(kind: LuaRuntimeErrorKind, description: impl Into<String>) -> LuaRuntimeError {
    LuaRuntimeError::new(kind, description)
}

pub(crate) fn handle_start_driver_lua_vm(world: &mut World, request: StartDriverLuaVm) {
    let config = world
        .get_resource::<LuaRuntimeConfig>()
        .cloned()
        .unwrap_or_default();
    let validation = (|| {
        if request.id.is_empty() {
            return Err(failure(
                LuaRuntimeErrorKind::InvalidRequest,
                "empty start ID",
            ));
        }
        if request.program.source.len() > config.max_source_bytes {
            return Err(failure(
                LuaRuntimeErrorKind::SourceTooLarge,
                "source limit exceeded",
            ));
        }
        let agent = world
            .get_component::<Agent>(request.owner)
            .ok_or_else(|| failure(LuaRuntimeErrorKind::InstanceNotFound, "Agent missing"))?;
        if !matches!(&agent.state, AgentState::Creating { vm_request_id, .. } if vm_request_id == &request.id)
        {
            return Err(failure(
                LuaRuntimeErrorKind::InstanceMismatch,
                "Creating operation mismatch",
            ));
        }
        if agent.lua.close.is_some() || agent.lua.instance.is_some() || agent.execution.is_closed()
        {
            return Err(failure(
                LuaRuntimeErrorKind::InvalidRequest,
                "Driver slots occupied or scope closed",
            ));
        }
        validate_environment(&request.environment)?;
        Ok(())
    })();
    if let Err(error) = validation {
        world.emit_event(StartDriverLuaVmResult {
            id: request.id,
            owner: request.owner,
            result: Err(error),
        });
        return;
    }
    let instance = DriverLuaVmInstance {
        owner: request.owner,
        start_request_id: request.id.clone(),
    };
    let (ready, mut ready_receiver) = DriverReadySlot::new(instance.clone());
    let (ended_sender, ended_receiver) = oneshot::channel();
    let close = DriverCloseHandle::new(instance.clone());
    let context = {
        let agent = world.get_component_mut::<Agent>(request.owner).unwrap();
        agent.lua.handled_exceptions.clear();
        agent.lua.instance = Some(instance.clone());
        agent.lua.ready = Some(ready.clone());
        agent.lua.ended = Some(DriverEndedSlot {
            instance: instance.clone(),
            receiver: ended_receiver,
        });
        agent.lua.close = Some(close.clone());
        LuaEnvironmentContext {
            instance: instance.clone(),
            values: BTreeMap::new(),
            wake: agent.lua.wake.clone(),
            execution: agent.execution.clone(),
            mcl: agent.mcl.drained.clone(),
        }
    };
    let (executed_sender, executed_receiver) = oneshot::channel();
    let program = request.program.clone();
    let environment = request.environment.clone();
    let driver_context = context.clone();
    let cancel = close.cancellation();
    let driver_config = config.clone();
    let submission = request.execution_entry.submit(Box::new(move || {
        Box::pin(async move {
            let result = AssertUnwindSafe(execute_lua(
                program,
                environment,
                driver_context,
                cancel,
                driver_config,
            ))
            .catch_unwind()
            .await;
            let result = result.unwrap_or_else(|_| {
                Err(failure(LuaRuntimeErrorKind::TaskFailed, "Driver panicked"))
            });
            let _ = executed_sender.send(result);
        })
    }));
    if let Err(error) = submission {
        let agent = world.get_component_mut::<Agent>(request.owner).unwrap();
        agent.lua.instance = None;
        agent.lua.ready = None;
        agent.lua.ended = None;
        agent.lua.close = None;
        world.emit_event(StartDriverLuaVmResult {
            id: request.id,
            owner: request.owner,
            result: Err(failure(LuaRuntimeErrorKind::TaskFailed, error.to_string())),
        });
        return;
    }
    let events = world.event_sender();
    world.spawn_async_service(supervise_driver(
        executed_receiver,
        ended_sender,
        ready,
        context.execution.clone(),
        context.mcl.clone(),
        context.wake.clone(),
        instance.clone(),
        events.clone(),
    ));
    let schedule = world
        .get_resource::<LuaRuntimeSchedule>()
        .unwrap()
        .0
        .clone();
    let mut guard = CoordinationGuard::start(events, request.clone(), close.clone(), context);
    world.send_async_closure::<DriverLuaVmStartOutput, DriverLuaVmStartTaskError, _, _>(
        &schedule,
        move || async move {
            if let Some(timeout) = config.startup_timeout {
                match tokio::time::timeout(timeout, &mut ready_receiver).await {
                    Err(_) => close.request_close(),
                    Ok(result) => {
                        guard.disarm();
                        let result = result.unwrap_or_else(|_| {
                            Err(failure(
                                LuaRuntimeErrorKind::CleanupFailed,
                                "ready sender lost without cleanup proof",
                            ))
                        });
                        return Ok(DriverLuaVmStartOutput {
                            request,
                            instance,
                            result,
                        });
                    }
                }
            }
            let result = ready_receiver.await.unwrap_or_else(|_| {
                Err(failure(
                    LuaRuntimeErrorKind::CleanupFailed,
                    "ready sender lost without cleanup proof",
                ))
            });
            guard.disarm();
            Ok(DriverLuaVmStartOutput {
                request,
                instance,
                result,
            })
        },
    );
}

async fn supervise_driver(
    executed: oneshot::Receiver<Result<(), LuaRuntimeError>>,
    ended: oneshot::Sender<Result<(), LuaRuntimeError>>,
    ready: DriverReadySlot,
    scope: AgentExecutionScope,
    mcl: AgentMclDrainHandle,
    wake: Arc<std::sync::Mutex<DriverWakeSlot>>,
    instance: DriverLuaVmInstance,
    events: RuntimeEventSender,
) {
    let execution = executed.await.unwrap_or_else(|_| {
        Err(failure(
            LuaRuntimeErrorKind::CleanupFailed,
            "Driver task dropped without exit proof",
        ))
    });
    scope.request_close();
    mcl.close();
    DriverWakeSlot::close(&wake);
    let (activities, receipts) = tokio::join!(scope.wait_all(), mcl.wait_drained());
    let cleanup = activities
        .and(receipts)
        .map_err(|error| failure(LuaRuntimeErrorKind::CleanupFailed, error.to_string()));
    let cleanup = if execution
        .as_ref()
        .err()
        .is_some_and(|error| error.kind == LuaRuntimeErrorKind::CleanupFailed)
    {
        execution.clone()
    } else {
        cleanup
    };
    let start_error = cleanup
        .as_ref()
        .err()
        .cloned()
        .or_else(|| execution.as_ref().err().cloned())
        .unwrap_or_else(|| failure(LuaRuntimeErrorKind::NotReady, "Driver ended before ready"));
    ready.complete(Err(start_error));
    if let Err(error) = execution {
        if error.kind != LuaRuntimeErrorKind::Cancelled {
            events.send_event(DriverLuaVmException {
                exception_id: format!("{}:execution", instance.start_request_id),
                instance: instance.clone(),
                error,
                execution_ended: true,
                cleanup_completed: cleanup.is_ok(),
            });
        }
    }
    let _ = ended.send(cleanup);
}

struct CoordinationGuard {
    events: RuntimeEventSender,
    start: Option<StartDriverLuaVm>,
    stop: Option<StopDriverLuaVm>,
    close: DriverCloseHandle,
    context: LuaEnvironmentContext,
    armed: bool,
}
impl CoordinationGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
    fn start(
        events: RuntimeEventSender,
        request: StartDriverLuaVm,
        close: DriverCloseHandle,
        context: LuaEnvironmentContext,
    ) -> Self {
        Self {
            events,
            start: Some(request),
            stop: None,
            close,
            context,
            armed: true,
        }
    }
}
impl Drop for CoordinationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.close.request_close();
        self.context.execution.request_close();
        self.context.mcl.close();
        DriverWakeSlot::close(&self.context.wake);
        let error = failure(
            LuaRuntimeErrorKind::CleanupFailed,
            "coordination task dropped; cleanup proof unavailable",
        );
        if let Some(request) = self.start.take() {
            self.events.send_event(StartDriverLuaVmResult {
                id: request.id,
                owner: request.owner,
                result: Err(error),
            });
        } else if let Some(request) = self.stop.take() {
            self.events.send_event(StopDriverLuaVmResult {
                id: request.id,
                owner: request.owner,
                instance: request.instance,
                result: Err(error),
            });
        }
    }
}

pub(crate) fn handle_driver_lua_vm_start_result(world: &mut World, output: DriverLuaVmStartOutput) {
    let Some(agent) = world.get_component_mut::<Agent>(output.request.owner) else {
        return;
    };
    if agent.lua.instance.as_ref() != Some(&output.instance)
        || agent.lua.ready.is_none()
        || !matches!(&agent.state, AgentState::Creating { vm_request_id, .. } if vm_request_id == &output.request.id)
    {
        return;
    }
    agent.lua.ready = None;
    world.emit_event(StartDriverLuaVmResult {
        id: output.request.id,
        owner: output.request.owner,
        result: output.result,
    });
}

pub(crate) fn handle_stop_driver_lua_vm(world: &mut World, request: StopDriverLuaVm) {
    let result = (|| {
        let agent = world
            .get_component_mut::<Agent>(request.owner)
            .ok_or_else(|| failure(LuaRuntimeErrorKind::InstanceNotFound, "Agent missing"))?;
        if agent.lua.instance.as_ref() != Some(&request.instance)
            || request.instance.owner != request.owner
        {
            return Err(failure(
                LuaRuntimeErrorKind::InstanceMismatch,
                "Driver instance mismatch",
            ));
        }
        let close = agent.lua.close.clone().ok_or_else(|| {
            failure(
                LuaRuntimeErrorKind::InstanceNotFound,
                "Driver close missing",
            )
        })?;
        let ended = agent.lua.ended.take().ok_or_else(|| {
            failure(
                LuaRuntimeErrorKind::StopAlreadyWaiting,
                "ended receiver already taken",
            )
        })?;
        close.request_close();
        DriverWakeSlot::close(&agent.lua.wake);
        agent.execution.request_close();
        agent.mcl.drained.close();
        let context = LuaEnvironmentContext {
            instance: request.instance.clone(),
            values: BTreeMap::new(),
            wake: agent.lua.wake.clone(),
            execution: agent.execution.clone(),
            mcl: agent.mcl.drained.clone(),
        };
        Ok((close, ended, context))
    })();
    let (close, ended, context) = match result {
        Ok(value) => value,
        Err(error) => {
            world.emit_event(StopDriverLuaVmResult {
                id: request.id,
                owner: request.owner,
                instance: request.instance,
                result: Err(error),
            });
            return;
        }
    };
    let schedule = world
        .get_resource::<LuaRuntimeSchedule>()
        .unwrap()
        .0
        .clone();
    let mut guard = CoordinationGuard {
        events: world.event_sender(),
        start: None,
        stop: Some(request.clone()),
        close,
        context,
        armed: true,
    };
    world.send_async_closure::<DriverLuaVmStopOutput, DriverLuaVmStopTaskError, _, _>(
        &schedule,
        move || async move {
            let result = ended.receiver.await.unwrap_or_else(|_| {
                Err(failure(
                    LuaRuntimeErrorKind::EndNotificationLost,
                    "ended sender lost",
                ))
            });
            guard.disarm();
            Ok(DriverLuaVmStopOutput {
                instance: request.instance.clone(),
                request,
                result,
            })
        },
    );
}
pub(crate) fn handle_driver_lua_vm_stop_result(world: &mut World, output: DriverLuaVmStopOutput) {
    if world
        .get_component::<Agent>(output.request.owner)
        .is_some_and(|agent| agent.lua.instance.as_ref() == Some(&output.instance)
            && matches!(&agent.state, AgentState::Stopping { vm_request_id: Some(id), .. } if id == &output.request.id))
        == false
    {
        return;
    }
    world.emit_event(StopDriverLuaVmResult {
        id: output.request.id,
        owner: output.request.owner,
        instance: output.instance,
        result: output.result,
    });
}
pub(crate) fn handle_driver_lua_vm_exception(world: &mut World, event: DriverLuaVmException) {
    let result = if let Some(agent) = world.get_component_mut::<Agent>(event.instance.owner) {
        if agent.lua.instance.as_ref() == Some(&event.instance) {
            if !agent
                .lua
                .handled_exceptions
                .insert(event.exception_id.clone())
            {
                return;
            }
            if let Some(close) = &agent.lua.close {
                close.request_close();
            }
            agent.execution.request_close();
            agent.mcl.drained.close();
            DriverWakeSlot::close(&agent.lua.wake);
            Ok(())
        } else {
            Err(failure(
                LuaRuntimeErrorKind::InstanceMismatch,
                "stale exception",
            ))
        }
    } else {
        Err(failure(
            LuaRuntimeErrorKind::InstanceNotFound,
            "exception owner missing",
        ))
    };
    let valid = result.is_ok();
    world.emit_event(DriverLuaVmExceptionResult {
        exception_id: event.exception_id,
        instance: event.instance,
        result,
        execution_ended: valid && event.execution_ended,
        cleanup_completed: valid && event.cleanup_completed,
    });
}

const MINIMAL_REMOVED_GLOBALS: [&str; 8] = [
    "dofile",
    "loadfile",
    "load",
    "loadstring",
    "require",
    "collectgarbage",
    "print",
    "warn",
];
fn create_minimal_lua() -> Result<Lua, LuaRuntimeError> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|error| failure(LuaRuntimeErrorKind::CompileFailed, error.to_string()))?;
    for name in MINIMAL_REMOVED_GLOBALS {
        lua.globals().set(name, MlValue::Nil).map_err(lua_error)?;
    }
    Ok(lua)
}
fn lua_error(error: mlua::Error) -> LuaRuntimeError {
    failure(LuaRuntimeErrorKind::ExecutionFailed, error.to_string())
}
fn validate_environment(environment: &LuaEnvironment) -> Result<(), LuaRuntimeError> {
    let mut names = std::collections::HashSet::new();
    for global in &environment.globals {
        if global.name.is_empty()
            || matches!(global.name.as_str(), "wait" | "mcl_wait")
            || !names.insert(global.name.clone())
        {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "invalid or duplicate global binding",
            ));
        }
    }
    let mut modules = std::collections::HashSet::new();
    for module in &environment.modules {
        if module.name.is_empty() || !modules.insert(module.name.clone()) {
            return Err(failure(
                LuaRuntimeErrorKind::EnvironmentFailed,
                "invalid or duplicate module",
            ));
        }
    }
    Ok(())
}
pub(crate) async fn execute_lua(
    program: LuaProgram,
    environment: LuaEnvironment,
    context: LuaEnvironmentContext,
    cancellation: CancellationToken,
    config: LuaRuntimeConfig,
) -> Result<(), LuaRuntimeError> {
    let lua = match program.libraries {
        LuaStandardLibraries::Minimal => create_minimal_lua()?,
        LuaStandardLibraries::Safe => {
            Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).map_err(lua_error)?
        }
        LuaStandardLibraries::Full => unsafe {
            Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default())
        },
    };
    lua.set_memory_limit(config.max_memory_bytes)
        .map_err(lua_error)?;
    let checkpoint = cancellation.clone();
    lua.set_hook(
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
    for binding in environment.globals {
        lua.globals()
            .set(
                binding.name,
                binding_value(&lua, binding.binding, &context, &cancellation)?,
            )
            .map_err(lua_error)?;
    }
    let loaded = lua.create_table().map_err(lua_error)?;
    for module in environment.modules {
        let table = lua.create_table().map_err(lua_error)?;
        for (name, binding) in module.exports {
            table
                .set(name, binding_value(&lua, binding, &context, &cancellation)?)
                .map_err(lua_error)?;
        }
        loaded.set(module.name, table).map_err(lua_error)?;
    }
    let modules = loaded.clone();
    lua.globals()
        .set(
            "require",
            lua.create_function(move |_, name: String| modules.get::<MlValue>(name))
                .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    let wake = context.wake.clone();
    let cancel = cancellation.clone();
    let wait = lua
        .create_async_function(move |_, ()| {
            let wake = wake.clone();
            let cancel = cancel.clone();
            async move {
                DriverWakeSlot::wait(&wake, &cancel)
                    .await
                    .map_err(mlua::Error::external)
            }
        })
        .map_err(lua_error)?;
    lua.globals().set("wait", wait.clone()).map_err(lua_error)?;
    lua.globals().set("mcl_wait", wait).map_err(lua_error)?;
    if cancellation.is_cancelled() {
        return Err(failure(
            LuaRuntimeErrorKind::Cancelled,
            "cancelled before execution",
        ));
    }
    let result = lua
        .load(program.source.as_ref())
        .set_name(program.origin.to_string_lossy())
        .exec_async()
        .await;
    if cancellation.is_cancelled() {
        return Err(failure(LuaRuntimeErrorKind::Cancelled, "Driver cancelled"));
    }
    result.map_err(lua_error)
}
fn binding_value(
    lua: &Lua,
    binding: LuaBindingValue,
    context: &LuaEnvironmentContext,
    cancellation: &CancellationToken,
) -> Result<MlValue, LuaRuntimeError> {
    match binding {
        LuaBindingValue::Value(value) => to_ml_value(lua, value),
        LuaBindingValue::Function(function) => {
            let context = context.clone();
            let cancellation = cancellation.clone();
            let callback = lua
                .create_async_function(move |lua, arguments: mlua::MultiValue| {
                    let function = function.clone();
                    let context = context.clone();
                    let cancellation = cancellation.clone();
                    async move {
                        let arguments = LuaValue::Array(
                            arguments
                                .into_iter()
                                .map(from_ml_value)
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(mlua::Error::external)?,
                        );
                        let value = function
                            .call(arguments, context, cancellation)
                            .await
                            .map_err(mlua::Error::external)?;
                        to_ml_value(&lua, value).map_err(mlua::Error::external)
                    }
                })
                .map_err(lua_error)?;
            Ok(MlValue::Function(callback))
        }
    }
}
fn to_ml_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError> {
    Ok(match value {
        LuaValue::Nil => MlValue::Nil,
        LuaValue::Boolean(value) => MlValue::Boolean(value),
        LuaValue::Integer(value) => MlValue::Integer(value),
        LuaValue::Number(value) => MlValue::Number(value),
        LuaValue::String(value) => MlValue::String(lua.create_string(value).map_err(lua_error)?),
        LuaValue::Array(values) => {
            let table = lua.create_table().map_err(lua_error)?;
            for (index, value) in values.into_iter().enumerate() {
                table
                    .set(index + 1, to_ml_value(lua, value)?)
                    .map_err(lua_error)?;
            }
            MlValue::Table(table)
        }
        LuaValue::Object(values) => {
            let table = lua.create_table().map_err(lua_error)?;
            for (key, value) in values {
                table
                    .set(key, to_ml_value(lua, value)?)
                    .map_err(lua_error)?;
            }
            MlValue::Table(table)
        }
    })
}
fn from_ml_value(value: MlValue) -> Result<LuaValue, LuaRuntimeError> {
    Ok(match value {
        MlValue::Nil => LuaValue::Nil,
        MlValue::Boolean(value) => LuaValue::Boolean(value),
        MlValue::Integer(value) => LuaValue::Integer(value),
        MlValue::Number(value) => LuaValue::Number(value),
        MlValue::String(value) => LuaValue::String(value.to_str().map_err(lua_error)?.to_owned()),
        MlValue::Table(table) => {
            let mut integers = BTreeMap::new();
            let mut object = BTreeMap::new();
            for pair in table.pairs::<MlValue, MlValue>() {
                let (key, value) = pair.map_err(lua_error)?;
                match key {
                    MlValue::Integer(index) if index > 0 => {
                        integers.insert(index, from_ml_value(value)?);
                    }
                    MlValue::String(key) => {
                        object.insert(
                            key.to_str().map_err(lua_error)?.to_owned(),
                            from_ml_value(value)?,
                        );
                    }
                    _ => {
                        return Err(failure(
                            LuaRuntimeErrorKind::ExecutionFailed,
                            "unsupported table key",
                        ))
                    }
                }
            }
            if !integers.is_empty() {
                if !object.is_empty() || integers.keys().copied().ne(1..=integers.len() as i64) {
                    return Err(failure(
                        LuaRuntimeErrorKind::ExecutionFailed,
                        "table is not a contiguous array",
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
                "value not serializable",
            ))
        }
    })
}
