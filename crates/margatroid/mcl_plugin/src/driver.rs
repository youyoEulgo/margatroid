use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use app_runtime_plugin::RuntimeEventSender;
use core_plugin::Entity;
use mlua::{Lua, LuaSerdeExt, Value};

use crate::{MclCommandReceived, MclCommandValue, MclDriverFailed, MclDriverSource};

static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(1);

/// Starts one isolated Lua VM for an Agent Base Driver.
///
/// The VM is deliberately not stored in an ECS component. Lua owns its own
/// stack and coroutine state on this thread; the only ECS boundary is the
/// command event and its one-shot reply channel.
pub fn spawn_base_driver(
    agent: Entity,
    source: MclDriverSource,
    events: RuntimeEventSender,
) -> Result<(), String> {
    let thread_name = format!("mcl-driver-{}", agent.index());
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(error) = run_driver(agent, source, events.clone()) {
                tracing::error!(?agent, error = %error, "MCL Base Driver failed");
                events.send_event(MclDriverFailed { agent, error });
            }
        })
        .map(|_| ())
        .map_err(|error| format!("MCL Driver thread could not start: {error}"))
}

fn run_driver(
    agent: Entity,
    source: MclDriverSource,
    events: RuntimeEventSender,
) -> Result<(), String> {
    let lua = Lua::new();
    let command_events = events.clone();
    let handle = lua
        .create_function(move |lua, (command, binding): (String, Option<Value>)| {
            let binding = binding
                .map(|value| lua.from_value::<serde_json::Value>(value))
                .transpose()
                .map_err(mlua::Error::external)?;
            let id = format!("mcl-{}", NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed));
            let (reply_tx, reply_rx) = mpsc::channel();
            command_events.send_event(MclCommandReceived {
                id,
                agent,
                command,
                binding,
                reply: reply_tx,
            });
            let value = reply_rx
                .recv()
                .map_err(|error| mlua::Error::external(error.to_string()))?
                .map_err(mlua::Error::external)?;
            match value {
                MclCommandValue::Unit => Ok(Value::Nil),
                MclCommandValue::Json(value) => lua.to_value(&value),
            }
        })
        .map_err(|error| error.to_string())?;
    lua.globals()
        .set("handle", handle)
        .map_err(|error| error.to_string())?;

    lua.load(source.source())
        .set_name(source.origin().to_string_lossy())
        .exec()
        .map_err(|error| error.to_string())
}
