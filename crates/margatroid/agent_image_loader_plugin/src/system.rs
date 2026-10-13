use async_runtime_plugin::AsyncTaskError;
use core_plugin::World;
use margatroid_types::agent_image_events::{AgentImageReadOutput, LoadAgentImage};

pub(crate) fn load_agent_image_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadAgentImage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_load_agent_image(world, request);
    }
    let outputs = world
        .event_reader::<Result<AgentImageReadOutput, AsyncTaskError>>()
        .into_iter()
        .filter_map(|output| match output {
            Ok(output) => Some(output.clone()),
            Err(error) => {
                tracing::error!(%error, "image async task stopped");
                None
            }
        })
        .collect::<Vec<_>>();
    for output in outputs {
        crate::handler::handle_mount_agent_image(world, output);
    }
}
