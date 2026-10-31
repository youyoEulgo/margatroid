use crate::handler::*;
use crate::types::{StartError, StartOutput, StopError, StopOutput};
use core_plugin::World;
use lua_runtime_core::{DriverLuaVmExited, StartDriverLuaVm, StopDriverLuaVm};

pub(crate) fn stop_driver_lua_vm_system(world: &mut World) {
    let requests = world
        .event_reader::<StopDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_stop_driver_lua_vm(world, request);
    }
    let results = world
        .event_reader::<Result<StopOutput, StopError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for result in results {
        handle_stop_result(world, result);
    }
}

pub(crate) fn start_driver_lua_vm_system(world: &mut World) {
    let requests = world
        .event_reader::<StartDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_start_driver_lua_vm(world, request);
    }
    let results = world
        .event_reader::<Result<StartOutput, StartError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for result in results {
        handle_start_result(world, result);
    }
}

pub(crate) fn driver_lua_vm_exit_system(world: &mut World) {
    let events = world
        .event_reader::<DriverLuaVmExited>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        handle_driver_lua_vm_exited(world, event);
    }
}
