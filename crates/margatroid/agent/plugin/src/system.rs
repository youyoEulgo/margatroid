use crate::handler;
use agent_core::{StartAgent, StopAgent};
use core_plugin::World;
use storage_core::AgentStorage;

pub(crate) fn start_agent_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<StartAgent<AgentStorage>>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handler::handle_start_agent(world, request);
    }
}

pub(crate) fn stop_agent_system(world: &mut World) {
    let requests: Vec<_> = world
        .event_reader::<StopAgent>()
        .into_iter()
        .cloned()
        .collect();
    for request in requests {
        handler::handle_stop_agent(world, request);
    }
}
