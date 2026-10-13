use core_plugin::World;

use crate::handler::{
    handle_load_sandbox_resource, handle_mount_sandbox_resource,
    handle_sandbox_resource_task_failed,
};
use margatroid_types::resource_events::{
    LoadSandboxResource, SandboxResourceReadOutput, SandboxResourceTaskError,
};
pub(crate) fn load_sandbox_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadSandboxResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let outputs = world
        .event_reader::<Result<SandboxResourceReadOutput, SandboxResourceTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_load_sandbox_resource(world, request);
    }
    for output in outputs {
        match output {
            Ok(output) => handle_mount_sandbox_resource(world, output),
            Err(error) => handle_sandbox_resource_task_failed(world, error),
        }
    }
}
