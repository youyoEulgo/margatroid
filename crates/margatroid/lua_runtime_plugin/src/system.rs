use core_plugin::World;

use crate::events::{LuaRuntimeCancelRequest, LuaRuntimeRequest, LuaRuntimeTaskFinished};
use crate::handler::{
    handle_lua_runtime_cancels, handle_lua_runtime_finished, handle_lua_runtime_request,
};

pub(crate) fn lua_runtime_request_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaRuntimeRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_lua_runtime_request(world, request);
    }
}

pub(crate) fn lua_runtime_cancel_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaRuntimeCancelRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    handle_lua_runtime_cancels(world, requests);
}

pub(crate) fn lua_runtime_result_system(world: &mut World) {
    let finished = world
        .event_reader::<LuaRuntimeTaskFinished>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    handle_lua_runtime_finished(world, finished);
}
