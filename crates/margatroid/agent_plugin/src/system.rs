use core_plugin::World;
use lua_runtime_plugin::{LuaRuntimeTaskFinished, LuaVmStarted};

use crate::{
    handle_agent_control, handle_agent_create, handle_agent_initialization_completed,
    handle_agent_message, handle_lua_vm_finished, handle_lua_vm_started, AgentControl,
    AgentCreateRequest, AgentInitializationCompleted, AgentMessage,
};

pub fn agent_create_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentCreateRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_agent_create(world, request);
    }
}

pub fn agent_control_system(world: &mut World) {
    let events = world
        .event_reader::<AgentControl>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        handle_agent_control(world, event);
    }
}

pub fn agent_message_system(world: &mut World) {
    let events = world
        .event_reader::<AgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        handle_agent_message(world, event);
    }
}

pub fn agent_lua_vm_state_system(world: &mut World) {
    let started = world
        .event_reader::<LuaVmStarted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in started {
        handle_lua_vm_started(world, event);
    }

    let completed = world
        .event_reader::<AgentInitializationCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in completed {
        handle_agent_initialization_completed(world, event);
    }

    let finished = world
        .event_reader::<LuaRuntimeTaskFinished>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in finished {
        handle_lua_vm_finished(world, event);
    }
}
