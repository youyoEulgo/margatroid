use crate::{handler, types::*};
use core_plugin::World;
use workspace_core::{RestartWorkspace, StartWorkspace, StopWorkspace};

pub(crate) fn stop_workspace_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<StopOutput, StopError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_stop_result(world, result);
    }
    for request in requests {
        handler::handle_stop_workspace(world, request);
    }
}
pub(crate) fn restart_workspace_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<RestartOutput, RestartError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<RestartWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_restart_result(world, result);
    }
    for request in requests {
        handler::handle_restart_workspace(world, request);
    }
}
pub(crate) fn start_workspace_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<StartOutput, StartError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_start_result(world, result);
    }
    for request in requests {
        handler::handle_start_workspace(world, request);
    }
}
