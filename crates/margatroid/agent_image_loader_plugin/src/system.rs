use core_plugin::World;

use crate::error::AgentImageTaskError;
use crate::events::LoadAgentImage;
use crate::handler::apply_agent_image_payload;
use crate::types::AgentImageReadOutput;

pub(crate) fn prepare_agent_image_load_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadAgentImage>()
        .into_iter()
        .map(|request| (request.id.clone(), request.reference.clone()))
        .collect::<Vec<_>>();

    for (id, reference) in requests {
        crate::handler::handle_agent_image_load(world, id, reference);
    }
}

pub(crate) fn apply_agent_image_load_system(world: &mut World) {
    let mut payloads = Vec::new();
    for output in world.event_reader::<Result<AgentImageReadOutput, AgentImageTaskError>>() {
        match output {
            Ok(output) => payloads.extend(output.take()),
            Err(error) => {
                tracing::error!(error = %error.source, "agent image loader async task stopped");
            }
        }
    }

    for payload in payloads {
        apply_agent_image_payload(world, payload);
    }
}
