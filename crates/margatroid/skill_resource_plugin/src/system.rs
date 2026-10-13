use core_plugin::World;

use crate::handler::{
    handle_load_skill_resource, handle_mount_skill_resource, handle_skill_resource_task_failed,
};
use margatroid_types::resource_events::{
    LoadSkillResource, SkillResourceReadOutput, SkillResourceTaskError,
};
pub(crate) fn load_skill_resource_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadSkillResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let outputs = world
        .event_reader::<Result<SkillResourceReadOutput, SkillResourceTaskError>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_load_skill_resource(world, request);
    }
    for output in outputs {
        match output {
            Ok(output) => handle_mount_skill_resource(world, output),
            Err(error) => handle_skill_resource_task_failed(world, error),
        }
    }
}
