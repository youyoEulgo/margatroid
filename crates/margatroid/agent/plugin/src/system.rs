use crate::{handler, types::*};
use agent_core::{AgentException, RestartAgent, StartAgent, StopAgent};
use core_plugin::World;

pub(crate) fn stop_agent_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<StopOutput, StopError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<StopAgent>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_stop_result(world, result);
    }
    for request in requests {
        handler::handle_stop_agent(world, request);
    }
}
pub(crate) fn restart_agent_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<RestartOutput, RestartError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<RestartAgent>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_restart_result(world, result);
    }
    for request in requests {
        handler::handle_restart_agent(world, request);
    }
}
pub(crate) fn start_agent_system(world: &mut World) {
    let results: Vec<_> = world
        .event_reader::<Result<StartOutput, StartError>>()
        .into_iter()
        .cloned()
        .collect();
    let requests: Vec<_> = world
        .event_reader::<StartAgent>()
        .into_iter()
        .cloned()
        .collect();
    for result in results {
        handler::handle_start_result(world, result);
    }
    for request in requests {
        handler::handle_start_agent(world, request);
    }
}
pub(crate) fn agent_exception_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<AgentException>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handler::handle_agent_exception(world, request);
    }
}
