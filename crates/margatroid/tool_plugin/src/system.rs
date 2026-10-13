use crate::ToolExecuteRequest;
use core_plugin::World;

pub(crate) fn tool_execute_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolExecuteRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_tool_execute(world, request);
    }
}
