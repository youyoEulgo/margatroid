use core_plugin::World;

use crate::handler::{
    handle_load_tool_resource, handle_mount_tool_resource, handle_tool_resource_task_failed,
};
use margatroid_types::resource_events::{
    LoadToolResource, ToolResourceReadOutput, ToolResourceTaskError,
};
pub(crate) fn load_tool_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadToolResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let outputs = world
        .event_reader::<Result<ToolResourceReadOutput, ToolResourceTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_load_tool_resource(world, request);
    }
    for output in outputs {
        match output {
            Ok(output) => handle_mount_tool_resource(world, output),
            Err(error) => handle_tool_resource_task_failed(world, error),
        }
    }
}
