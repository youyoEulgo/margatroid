use crate::*;
use core_plugin::World;
use margatroid_types::lua_runtime_events::{
    DriverLuaVmExceptionResult, StartDriverLuaVmResult, StopDriverLuaVmResult,
};

pub fn start_agent_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<StartAgent>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handle_start_agent(world, request);
    }
    let database_results: Vec<_> = world
        .event_reader::<Result<DatabaseSetupOutput, DatabaseSetupError>>()
        .into_iter()
        .cloned()
        .collect();
    for result in database_results {
        handle_database_setup_result(world, result);
    }
    let results: Vec<_> = world
        .event_reader::<StartDriverLuaVmResult>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handle_start_driver_lua_vm_result(world, result);
    }
}
pub fn stop_agent_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<StopAgent>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handle_stop_agent(world, request);
    }
    let results: Vec<_> = world
        .event_reader::<StopDriverLuaVmResult>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handle_stop_driver_lua_vm_result(world, result);
    }
}
pub fn agent_exception_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<DriverLuaVmExceptionResult>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handle_driver_lua_vm_exception_result(world, result);
    }
    let requests: Vec<_> = world
        .event_reader::<AgentException>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handle_agent_exception(world, request);
    }
}
