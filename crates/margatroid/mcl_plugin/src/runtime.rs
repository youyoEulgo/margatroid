use crate::queue::*;
use app_runtime_plugin::{RuntimeEventSender, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin, Resource, World};
use margatroid_types::agent_components::Agent;
use margatroid_types::lua_environment::*;
use margatroid_types::lua_runtime_components::{
    DriverLuaVmInstance, LuaRuntimeError, LuaRuntimeErrorKind,
};
use margatroid_types::mcl_environment::MclEnvironmentFactory;
use margatroid_types::mcl_events::*;
use resource_id_plugin::ResourceID;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub struct MclPlugin {
    pub schedule: String,
}
impl MclPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
impl Default for MclPlugin {
    fn default() -> Self {
        Self::new()
    }
}
pub struct MclPluginInstalled;
impl Resource for MclPluginInstalled {}

#[derive(Default)]
struct HostReplies(BTreeMap<(String, ResourceID, ResourceID), HostReply>);
struct HostReply {
    instance: DriverLuaVmInstance,
    cancel: CancellationToken,
    sender: oneshot::Sender<MclCommandResult>,
}
impl Resource for HostReplies {}

impl Plugin for MclPlugin {
    fn build(self, app: &mut App) {
        if !app.contains_schedule(&self.schedule)
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            panic!("MclPlugin requires runtime schedule and AsyncRuntimePlugin");
        }
        let events = app.world().event_sender();
        app.world_mut()
            .insert_resource(MclEnvironmentFactory(Arc::new(move || {
                environment(events.clone())
            })));
        app.world_mut().insert_resource(HostReplies::default());
        app.world_mut().insert_resource(MclPluginInstalled);
        app.add_system(&self.schedule, mcl_command_system);
    }
}

pub fn mcl_command_system(world: &mut World) {
    let host_requests = world
        .event_reader::<InternalMclHostSubmission>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let requests = world
        .event_reader::<MclCommand>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for host in host_requests {
        let sender = host.reply.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(sender) = sender else {
            continue;
        };
        let source = world
            .get_component::<Agent>(host.instance.owner)
            .filter(|a| a.lua.instance.as_ref() == Some(&host.instance))
            .map(|a| a.resourceid.clone());
        let Some(source) = source else {
            let _ = sender.send(MclCommandResult {
                id: host.id,
                source: host.target.clone(),
                target: host.target,
                result: Err(MclError::AgentMissing),
                history_error: None,
            });
            continue;
        };
        if let Some(replies) = world.get_resource_mut::<HostReplies>() {
            replies.0.insert(
                (host.id.clone(), source.clone(), host.target.clone()),
                HostReply {
                    instance: host.instance.clone(),
                    cancel: host.cancel.clone(),
                    sender,
                },
            );
        }
        enqueue_mcl_command(
            world,
            MclCommand {
                id: host.id,
                source,
                target: host.target,
                command: host.command,
            },
        );
    }
    for request in requests {
        enqueue_mcl_command(world, request);
    }
    let outputs = world
        .event_reader::<MclAsyncOutput>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for output in outputs {
        complete_mcl_async(world, output);
    }
    let cancelled = world
        .event_reader::<MclCancelledOutput>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for output in cancelled {
        complete_cancelled(world, output);
    }
    let entities = world.query_with::<Agent>().result();
    for entity in entities {
        process_mcl_queue(world, entity);
    }
}

pub(crate) fn publish_result(world: &mut World, result: MclCommandResult) {
    if let Some(reply) = world.get_resource_mut::<HostReplies>().and_then(|r| {
        r.0.remove(&(
            result.id.clone(),
            result.source.clone(),
            result.target.clone(),
        ))
    }) {
        let _ = reply.sender.send(result.clone());
    }
    world.send_event(result);
}

pub(crate) fn driver_command_cancelled(world: &World, request: &MclCommand) -> bool {
    let key = (
        request.id.clone(),
        request.source.clone(),
        request.target.clone(),
    );
    let Some(reply) = world
        .get_resource::<HostReplies>()
        .and_then(|r| r.0.get(&key))
    else {
        return false;
    };
    reply.cancel.is_cancelled()
        || world
            .get_component::<Agent>(reply.instance.owner)
            .is_none_or(|agent| {
                agent.lua.instance.as_ref() != Some(&reply.instance)
                    || agent.execution.is_closed()
                    || matches!(
                        agent.state,
                        margatroid_types::agent_components::AgentState::Stopping { .. }
                    )
            })
}

pub(crate) fn driver_ready_authorized(
    world: &World,
    entity: core_plugin::Entity,
    request: &MclCommand,
) -> bool {
    let key = (
        request.id.clone(),
        request.source.clone(),
        request.target.clone(),
    );
    let Some(reply) = world
        .get_resource::<HostReplies>()
        .and_then(|r| r.0.get(&key))
    else {
        return false;
    };
    reply.instance.owner == entity
        && world.get_component::<Agent>(entity).is_some_and(|agent| {
            agent.lua.instance.as_ref() == Some(&reply.instance)
                && agent
                    .lua
                    .ready
                    .as_ref()
                    .is_none_or(|slot| slot.instance == reply.instance)
                && matches!(
                    agent.state,
                    margatroid_types::agent_components::AgentState::Creating { .. }
                        | margatroid_types::agent_components::AgentState::Running
                )
        })
}

fn environment(events: RuntimeEventSender) -> LuaEnvironment {
    LuaEnvironment {
        globals: vec![LuaGlobalBinding {
            name: "mcl".into(),
            binding: LuaBindingValue::Function(Arc::new(MclHost { events })),
        }],
        modules: vec![],
    }
}

fn lua_error(kind: LuaRuntimeErrorKind, message: impl Into<String>) -> LuaRuntimeError {
    LuaRuntimeError::new(kind, message)
}

struct MclHost {
    events: RuntimeEventSender,
}
impl LuaHostFunction for MclHost {
    fn call(
        &self,
        arguments: LuaValue,
        context: LuaEnvironmentContext,
        cancel: CancellationToken,
    ) -> HostFuture {
        let events = self.events.clone();
        Box::pin(async move {
            if cancel.is_cancelled() || context.execution.is_closed() {
                return Err(lua_error(LuaRuntimeErrorKind::Cancelled, "driver closed"));
            }
            let LuaValue::Array(values) = arguments else {
                return Err(lua_error(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "mcl expects target, text, optional arguments array",
                ));
            };
            if !(2..=3).contains(&values.len()) {
                return Err(lua_error(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "mcl expects target, text, optional arguments array",
                ));
            }
            let LuaValue::String(target) = &values[0] else {
                return Err(lua_error(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "target must be resource ID",
                ));
            };
            let target = target
                .parse()
                .map_err(|_| lua_error(LuaRuntimeErrorKind::InvalidRequest, "invalid target"))?;
            let LuaValue::String(text) = &values[1] else {
                return Err(lua_error(
                    LuaRuntimeErrorKind::InvalidRequest,
                    "command must be text",
                ));
            };
            let manual = decode_host_arguments(values.get(2))?;
            let id = next_host_id()?;
            let (tx, mut rx) = oneshot::channel();
            let source_cancel = context.execution.child_cancel();
            events.send_event(InternalMclHostSubmission {
                id,
                instance: context.instance,
                cancel: source_cancel.clone(),
                target,
                command: MclInstruction {
                    text: text.clone(),
                    arguments: manual,
                },
                reply: Arc::new(Mutex::new(Some(tx))),
            });
            let response = tokio::select! {
                response = &mut rx => response,
                _ = source_cancel.cancelled() => {
                    events.send_event(InternalMclCancellationSignal);
                    rx.await
                },
                _ = cancel.cancelled() => {
                    source_cancel.cancel();
                    events.send_event(InternalMclCancellationSignal);
                    rx.await
                },
            }
            .map_err(|e| lua_error(LuaRuntimeErrorKind::TaskFailed, e.to_string()))?;
            let value = response
                .result
                .map_err(|e| lua_error(LuaRuntimeErrorKind::ExecutionFailed, e.to_string()))?;
            json_to_lua(
                margatroid_types::mcl_projection::project_mcl_value(&value)
                    .map_err(|e| lua_error(LuaRuntimeErrorKind::TaskFailed, e.to_string()))?,
            )
        })
    }
}
fn next_host_id() -> Result<String, LuaRuntimeError> {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let session = SESSION.get_or_init(|| {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_nanos();
        format!("{}:{epoch}", std::process::id())
    });
    let sequence = NEXT
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |value| value.checked_add(1),
        )
        .map_err(|_| lua_error(LuaRuntimeErrorKind::TaskFailed, "MCL request ID exhausted"))?;
    Ok(format!("lua-mcl:{session}:{sequence}"))
}

fn decode_host_arguments(
    value: Option<&LuaValue>,
) -> Result<Vec<serde_json::Value>, LuaRuntimeError> {
    let arrays = match value {
        None => return Ok(vec![]),
        Some(LuaValue::Object(object)) if object.is_empty() => return Ok(vec![]),
        Some(LuaValue::Array(arrays)) => arrays,
        _ => {
            return Err(lua_error(
                LuaRuntimeErrorKind::InvalidRequest,
                "arguments must retain outer array",
            ))
        }
    };
    arrays
        .iter()
        .map(|array| match array {
            LuaValue::Object(object) if object.is_empty() => Ok(serde_json::Value::Array(vec![])),
            LuaValue::Array(_) => lua_to_json(array.clone()),
            _ => Err(lua_error(
                LuaRuntimeErrorKind::InvalidRequest,
                "each manual parameter must be an array",
            )),
        })
        .collect()
}

fn lua_to_json(value: LuaValue) -> Result<serde_json::Value, LuaRuntimeError> {
    Ok(match value {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(v) => v.into(),
        LuaValue::Integer(v) => v.into(),
        LuaValue::Number(v) => serde_json::Number::from_f64(v)
            .ok_or_else(|| lua_error(LuaRuntimeErrorKind::InvalidRequest, "nonfinite number"))?
            .into(),
        LuaValue::String(v) => v.into(),
        LuaValue::Array(v) => {
            serde_json::Value::Array(v.into_iter().map(lua_to_json).collect::<Result<_, _>>()?)
        }
        LuaValue::Object(v) => serde_json::Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, lua_to_json(v)?)))
                .collect::<Result<_, LuaRuntimeError>>()?,
        ),
    })
}
fn json_to_lua(value: serde_json::Value) -> Result<LuaValue, LuaRuntimeError> {
    Ok(match value {
        serde_json::Value::Null => LuaValue::Nil,
        serde_json::Value::Bool(v) => LuaValue::Boolean(v),
        serde_json::Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                LuaValue::Integer(v)
            } else {
                LuaValue::Number(
                    v.as_f64().ok_or_else(|| {
                        lua_error(LuaRuntimeErrorKind::TaskFailed, "invalid number")
                    })?,
                )
            }
        }
        serde_json::Value::String(v) => LuaValue::String(v),
        serde_json::Value::Array(v) => {
            LuaValue::Array(v.into_iter().map(json_to_lua).collect::<Result<_, _>>()?)
        }
        serde_json::Value::Object(v) => LuaValue::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, json_to_lua(v)?)))
                .collect::<Result<_, LuaRuntimeError>>()?,
        ),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_ids_include_process_epoch_and_monotonic_sequence() {
        let first = super::next_host_id().unwrap();
        let second = super::next_host_id().unwrap();
        let a = first.split(':').collect::<Vec<_>>();
        let b = second.split(':').collect::<Vec<_>>();
        assert_eq!(a.len(), 4);
        assert_eq!(a[0], "lua-mcl");
        assert_eq!(a[1].parse::<u32>().unwrap(), std::process::id());
        assert!(a[2].parse::<u128>().unwrap() > 0);
        assert_eq!(a[..3], b[..3]);
        assert!(b[3].parse::<u64>().unwrap() > a[3].parse::<u64>().unwrap());
    }
}
