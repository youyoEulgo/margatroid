use crate::types::{ToolLaunchError, ToolLaunchOutput};
use core_plugin::World;
use tool_core::{InitToolTasks, ToolExecuteRequest};

pub(crate) fn init_tool_tasks_system(world: &mut World) {
    let requests = world
        .event_reader::<InitToolTasks>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_init_tool_tasks(world, request);
    }
}

pub(crate) fn tool_execute_system(world: &mut World) {
    for result in world.event_reader::<Result<ToolLaunchOutput, ToolLaunchError>>() {
        if let Err(failure) = result {
            if let Some(fallback) = &failure.fallback {
                fallback.fail(failure.error.clone());
            }
        }
    }
    let requests = world
        .event_reader::<ToolExecuteRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_tool_execute(world, request);
    }
}
