use core_plugin::World;

use crate::handler::{
    handle_hook_resource_task_failed, handle_load_hook_resource, handle_mount_hook_resource,
};
use margatroid_types::resource_events::{
    HookResourceReadOutput, HookResourceTaskError, LoadHookResource,
};
pub(crate) fn load_hook_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadHookResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let outputs = world
        .event_reader::<Result<HookResourceReadOutput, HookResourceTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_load_hook_resource(world, request);
    }
    for output in outputs {
        match output {
            Ok(output) => handle_mount_hook_resource(world, output),
            Err(error) => handle_hook_resource_task_failed(world, error),
        }
    }
}
