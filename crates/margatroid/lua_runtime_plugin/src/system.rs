use crate::handler::*;
use core_plugin::World;
use margatroid_types::lua_runtime_events::*;

pub(crate) fn start_driver_lua_vm_system(world: &mut World) {
    let requests = world
        .event_reader::<StartDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_start_driver_lua_vm(world, request);
    }
    let outputs = world
        .event_reader::<Result<DriverLuaVmStartOutput, DriverLuaVmStartTaskError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().ok().cloned())
        .collect::<Vec<_>>();
    for output in outputs {
        handle_driver_lua_vm_start_result(world, output);
    }
}
pub(crate) fn stop_driver_lua_vm_system(world: &mut World) {
    let requests = world
        .event_reader::<StopDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_stop_driver_lua_vm(world, request);
    }
    let outputs = world
        .event_reader::<Result<DriverLuaVmStopOutput, DriverLuaVmStopTaskError>>()
        .into_iter()
        .filter_map(|result| result.as_ref().ok().cloned())
        .collect::<Vec<_>>();
    for output in outputs {
        handle_driver_lua_vm_stop_result(world, output);
    }
}
pub(crate) fn driver_lua_vm_exception_system(world: &mut World) {
    let events = world
        .event_reader::<DriverLuaVmException>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        handle_driver_lua_vm_exception(world, event);
    }
}
