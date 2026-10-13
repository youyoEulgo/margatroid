use core_plugin::World;

use crate::handler::{
    handle_load_prompt_resource, handle_mount_prompt_resource, handle_prompt_resource_task_failed,
};
use margatroid_types::resource_events::{
    LoadPromptResource, PromptResourceReadOutput, PromptResourceTaskError,
};
pub(crate) fn load_prompt_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadPromptResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let outputs = world
        .event_reader::<Result<PromptResourceReadOutput, PromptResourceTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_load_prompt_resource(world, request);
    }
    for output in outputs {
        match output {
            Ok(output) => handle_mount_prompt_resource(world, output),
            Err(error) => handle_prompt_resource_task_failed(world, error),
        }
    }
}
