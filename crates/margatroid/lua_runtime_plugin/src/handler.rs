use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use core_plugin::World;
use margatroid_types::LuaVmId;
use mlua::{Lua, LuaOptions, StdLib, Value as MlValue};

use crate::error::LuaRuntimeError;
use crate::events::{
    LuaRuntimeCancelRequest, LuaRuntimeRequest, LuaRuntimeTaskFinished, LuaVmMessage,
    LuaVmMessageReceiveRequest, LuaVmMessageReceived, LuaVmStarted,
};
use crate::types::{
    CancellationToken, LuaEnvironmentContext, LuaEnvironmentRegistry, LuaProgram, LuaRuntimeConfig,
    LuaRuntimeHandle, LuaRuntimeResult, LuaRuntimeState, LuaScheduler, LuaValue, LuaVmSession,
    LuaVmState,
};

pub(crate) fn handle_lua_runtime_request(world: &mut World, request: LuaRuntimeRequest) {
    let max_source = world
        .get_resource::<LuaRuntimeConfig>()
        .map_or(1024 * 1024, |config| config.max_source_bytes);
    let runtime = world.get_resource::<LuaRuntimeHandle>().cloned();
    if request.request_id.is_empty() {
        let error = LuaRuntimeError::InvalidRequest("request id is empty".into());
        request.reply.fail(error.clone());
        world.emit_event(LuaRuntimeTaskFinished {
            request_id: request.request_id,
            vm_id: None,
            owner: request.owner,
            state: LuaVmState::Failed,
            error: Some(error),
        });
        return;
    }
    let duplicate = world
        .get_resource::<LuaRuntimeState>()
        .is_some_and(|state| state.requests.contains_key(&request.request_id));
    if duplicate {
        let error = LuaRuntimeError::InvalidRequest("request id is already running".into());
        request.reply.fail(error.clone());
        world.emit_event(LuaRuntimeTaskFinished {
            request_id: request.request_id,
            vm_id: None,
            owner: request.owner,
            state: LuaVmState::Failed,
            error: Some(error),
        });
        return;
    }
    if request.program.source.len() > max_source {
        request.reply.fail(LuaRuntimeError::SourceTooLarge);
        world.emit_event(LuaRuntimeTaskFinished {
            request_id: request.request_id,
            vm_id: None,
            owner: request.owner,
            state: LuaVmState::Failed,
            error: Some(LuaRuntimeError::SourceTooLarge),
        });
        return;
    }
    let (vm_id, cancel) = {
        let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
            request.reply.fail(LuaRuntimeError::RuntimeClosed);
            return;
        };
        let vm_id = LuaVmId(state.next_vm.fetch_add(1, Ordering::Relaxed) + 1);
        let now = Instant::now();
        state.sessions.insert(
            vm_id,
            LuaVmSession {
                vm_id,
                owner: request.owner.clone(),
                state: LuaVmState::Running,
                created_at: now,
                last_activity: now,
            },
        );
        state.mailboxes.entry(vm_id).or_default();
        let cancel = CancellationToken::default();
        state
            .requests
            .insert(request.request_id.clone(), cancel.clone());
        state
            .owners
            .insert(request.request_id.clone(), request.owner.owner_id.clone());
        (vm_id, cancel)
    };
    let owner = request.owner.clone();
    let request_id = request.request_id.clone();
    let reply = request.reply.clone();
    let events = runtime.as_ref().map(|handle| handle.events.clone());
    let registry = runtime
        .as_ref()
        .map(|handle| Arc::clone(&handle.environments));
    if request.scheduler == LuaScheduler::LongRunning {
        if let Some(sender) = &events {
            sender.send_event(LuaVmStarted {
                request_id: request_id.clone(),
                vm_id,
                owner: owner.clone(),
            });
        }
    }
    let program = request.program;
    let context = request.context;
    let providers = request.providers;
    let deadline = request.deadline;
    let max_result = world
        .get_resource::<LuaRuntimeConfig>()
        .map_or(4 * 1024 * 1024, |config| config.max_result_bytes);
    std::thread::spawn(move || {
        let result = if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Err(LuaRuntimeError::Timeout)
        } else {
            execute_lua(program, context, providers, registry, cancel.clone()).and_then(|value| {
                (lua_value_size(&value) <= max_result)
                    .then_some(value)
                    .ok_or(LuaRuntimeError::ResultTooLarge)
            })
        };
        if cancel.is_cancelled() {
            if let Some(sender) = events {
                sender.send_event(LuaRuntimeTaskFinished {
                    request_id,
                    vm_id: Some(vm_id),
                    owner,
                    state: LuaVmState::Cancelled,
                    error: None,
                });
            }
            let _ = reply
                .take()
                .map(|sender| sender.send(LuaRuntimeResult::Cancelled));
        } else {
            let state = if result.is_ok() {
                LuaVmState::Completed
            } else {
                LuaVmState::Failed
            };
            if let Some(sender) = events {
                sender.send_event(LuaRuntimeTaskFinished {
                    request_id,
                    vm_id: Some(vm_id),
                    owner,
                    state,
                    error: result.as_ref().err().cloned(),
                });
            }
            let _ = reply.take().map(|sender| {
                sender.send(
                    result
                        .map(|value| LuaRuntimeResult::Completed { value })
                        .unwrap_or_else(|error| LuaRuntimeResult::Failed { error }),
                )
            });
        }
    });
}

pub(crate) fn handle_lua_runtime_cancels(
    world: &mut World,
    requests: Vec<LuaRuntimeCancelRequest>,
) {
    let Some(state) = world.get_resource::<LuaRuntimeState>() else {
        return;
    };
    for request in requests {
        if let Some(token) = state.requests.get(&request.request_id) {
            token.cancel();
        }
        for (request_id, owner_id) in &state.owners {
            if owner_id == &request.request_id {
                if let Some(token) = state.requests.get(request_id) {
                    token.cancel();
                }
            }
        }
    }
}

pub(crate) fn handle_lua_vm_messages(world: &mut World, messages: Vec<LuaVmMessage>) {
    let mut responses = Vec::new();
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    for message in messages {
        if let Some(waiting) = state
            .receives
            .get_mut(&message.vm_id)
            .and_then(VecDeque::pop_front)
        {
            responses.push(LuaVmMessageReceived {
                id: waiting.id,
                vm_id: message.vm_id,
                result: Ok(message.value),
            });
        } else if let Some(mailbox) = state.mailboxes.get_mut(&message.vm_id) {
            mailbox.push_back(message.value);
        }
    }
    for response in responses {
        world.emit_event(response);
    }
}

pub(crate) fn handle_lua_vm_receives(world: &mut World, requests: Vec<LuaVmMessageReceiveRequest>) {
    let mut responses = Vec::new();
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    for request in requests {
        match state
            .mailboxes
            .get_mut(&request.vm_id)
            .and_then(VecDeque::pop_front)
        {
            Some(value) => responses.push(LuaVmMessageReceived {
                id: request.id,
                vm_id: request.vm_id,
                result: Ok(value),
            }),
            None if state.sessions.contains_key(&request.vm_id) => {
                let receives = state.receives.entry(request.vm_id).or_default();
                if receives.is_empty() {
                    receives.push_back(request);
                } else {
                    responses.push(LuaVmMessageReceived {
                        id: request.id,
                        vm_id: request.vm_id,
                        result: Err(LuaRuntimeError::InvalidRequest(
                            "only one receive may wait on a VM mailbox".into(),
                        )),
                    });
                }
            }
            None => responses.push(LuaVmMessageReceived {
                id: request.id,
                vm_id: request.vm_id,
                result: Err(LuaRuntimeError::VmExecutionFailed(
                    "VM is not running".into(),
                )),
            }),
        }
    }
    for response in responses {
        world.emit_event(response);
    }
}

pub(crate) fn handle_lua_runtime_finished(
    world: &mut World,
    finished: Vec<LuaRuntimeTaskFinished>,
) {
    let Some(state) = world.get_resource_mut::<LuaRuntimeState>() else {
        return;
    };
    let mut receive_failures = Vec::new();
    for event in finished {
        state.requests.remove(&event.request_id);
        state.owners.remove(&event.request_id);
        if let Some(vm_id) = event.vm_id {
            state.sessions.remove(&vm_id);
            state.mailboxes.remove(&vm_id);
            if let Some(receives) = state.receives.remove(&vm_id) {
                for receive in receives {
                    receive_failures.push(LuaVmMessageReceived {
                        id: receive.id,
                        vm_id,
                        result: Err(event
                            .error
                            .clone()
                            .unwrap_or(LuaRuntimeError::RuntimeClosed)),
                    });
                }
            }
        }
    }
    for response in receive_failures {
        world.emit_event(response);
    }
}

fn execute_lua(
    program: LuaProgram,
    context: LuaEnvironmentContext,
    providers: Vec<String>,
    registry: Option<Arc<RwLock<LuaEnvironmentRegistry>>>,
    cancellation: CancellationToken,
) -> Result<LuaValue, LuaRuntimeError> {
    let lua = match program.libraries {
        crate::types::LuaStandardLibraries::Safe => {
            Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
                .map_err(|error| LuaRuntimeError::VmCreationFailed(error.to_string()))?
        }
        crate::types::LuaStandardLibraries::Full => unsafe {
            Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default())
        },
    };
    if let Some(registry) = registry {
        let env = registry
            .read()
            .map_err(|_| LuaRuntimeError::RuntimeClosed)?
            .collect(&providers, &context)?;
        for binding in env.globals {
            match binding.binding {
                crate::types::LuaBindingValue::Value(value) => {
                    lua.globals()
                        .set(binding.name, to_ml_value(&lua, value)?)
                        .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
                }
                crate::types::LuaBindingValue::Function(function) => {
                    let context = context.clone();
                    let cancellation = cancellation.clone();
                    let callback = lua
                        .create_function(move |lua, arguments: mlua::MultiValue| {
                            let arguments = LuaValue::Array(
                                arguments
                                    .into_iter()
                                    .map(from_ml_value)
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(mlua::Error::external)?,
                            );
                            let context = context.clone();
                            let function = function.clone();
                            let cancellation = cancellation.clone();
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(mlua::Error::external)?;
                            let value = runtime
                                .block_on(function.call(arguments, context, cancellation))
                                .map_err(mlua::Error::external)?;
                            to_ml_value(lua, value).map_err(mlua::Error::external)
                        })
                        .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
                    lua.globals()
                        .set(binding.name, callback)
                        .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
                }
            }
        }
        for module in env.modules {
            let table = lua
                .create_table()
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            for (name, binding) in module.exports {
                let value = match binding {
                    crate::types::LuaBindingValue::Value(value) => to_ml_value(&lua, value)?,
                    crate::types::LuaBindingValue::Function(function) => {
                        let context = context.clone();
                        let cancellation = cancellation.clone();
                        MlValue::Function(
                            lua.create_function(move |lua, arguments: mlua::MultiValue| {
                                let arguments = LuaValue::Array(
                                    arguments
                                        .into_iter()
                                        .map(from_ml_value)
                                        .collect::<Result<Vec<_>, _>>()
                                        .map_err(mlua::Error::external)?,
                                );
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .map_err(mlua::Error::external)?;
                                let value = runtime
                                    .block_on(function.call(
                                        arguments,
                                        context.clone(),
                                        cancellation.clone(),
                                    ))
                                    .map_err(mlua::Error::external)?;
                                to_ml_value(lua, value).map_err(mlua::Error::external)
                            })
                            .map_err(|error| {
                                LuaRuntimeError::VmExecutionFailed(error.to_string())
                            })?,
                        )
                    }
                };
                table
                    .set(name, value)
                    .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            }
            let package = lua
                .globals()
                .get::<mlua::Table>("package")
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            let loaded = package
                .get::<mlua::Table>("loaded")
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            loaded
                .set(module.name, table)
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
        }
    }
    let chunk = lua.load(&program.source);
    if cancellation.is_cancelled() {
        return Err(LuaRuntimeError::Cancelled);
    }
    let value = if let Some(entry) = program.entry {
        chunk
            .exec()
            .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
        lua.globals()
            .get::<MlValue>(entry)
            .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?
    } else {
        chunk
            .eval::<MlValue>()
            .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?
    };
    from_ml_value(value)
}

fn lua_value_size(value: &LuaValue) -> usize {
    match value {
        LuaValue::Nil => 1,
        LuaValue::Boolean(_) => 1,
        LuaValue::Integer(_) | LuaValue::Number(_) => 8,
        LuaValue::String(value) => value.len(),
        LuaValue::Array(values) => values.iter().map(lua_value_size).sum(),
        LuaValue::Object(values) => values
            .iter()
            .map(|(key, value)| key.len() + lua_value_size(value))
            .sum(),
    }
}

fn to_ml_value(lua: &Lua, value: LuaValue) -> Result<MlValue, LuaRuntimeError> {
    Ok(match value {
        LuaValue::Nil => MlValue::Nil,
        LuaValue::Boolean(value) => MlValue::Boolean(value),
        LuaValue::Integer(value) => MlValue::Integer(value),
        LuaValue::Number(value) => MlValue::Number(value),
        LuaValue::String(value) => MlValue::String(
            lua.create_string(&value)
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?,
        ),
        LuaValue::Array(values) => {
            let table = lua
                .create_table()
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            for (index, value) in values.into_iter().enumerate() {
                table
                    .set(index + 1, to_ml_value(lua, value)?)
                    .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            }
            MlValue::Table(table)
        }
        LuaValue::Object(values) => {
            let table = lua
                .create_table()
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
            for (key, value) in values {
                table
                    .set(key, to_ml_value(lua, value)?)
                    .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
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
        MlValue::String(value) => LuaValue::String(
            value
                .to_str()
                .map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?
                .to_owned(),
        ),
        MlValue::Table(table) => {
            let mut array = Vec::new();
            let mut object = BTreeMap::new();
            for pair in table.pairs::<MlValue, MlValue>() {
                let (key, value) =
                    pair.map_err(|error| LuaRuntimeError::VmExecutionFailed(error.to_string()))?;
                match key {
                    MlValue::Integer(index) if index > 0 => {
                        if index as usize != array.len() + 1 {
                            return Err(LuaRuntimeError::VmExecutionFailed(
                                "result table is not a contiguous array".into(),
                            ));
                        }
                        array.push(from_ml_value(value)?);
                    }
                    MlValue::String(key) => {
                        object.insert(
                            key.to_str()
                                .map_err(|error| {
                                    LuaRuntimeError::VmExecutionFailed(error.to_string())
                                })?
                                .to_owned(),
                            from_ml_value(value)?,
                        );
                    }
                    _ => {
                        return Err(LuaRuntimeError::VmExecutionFailed(
                            "result table key is unsupported".into(),
                        ))
                    }
                }
            }
            if !array.is_empty() && object.is_empty() {
                LuaValue::Array(array)
            } else {
                LuaValue::Object(object)
            }
        }
        _ => {
            return Err(LuaRuntimeError::VmExecutionFailed(
                "result value is not serializable".into(),
            ))
        }
    })
}
