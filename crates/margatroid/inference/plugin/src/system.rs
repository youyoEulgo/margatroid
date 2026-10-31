use core_plugin::World;
use inference_core::{InferenceRequest, InitInference};

pub(crate) fn init_inference_system(world: &mut World) {
    let requests = world
        .event_reader::<InitInference>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_init_inference(world, request);
    }
}

pub(crate) fn inference_system(world: &mut World) {
    let requests = world
        .event_reader::<InferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_inference(world, request);
    }
}
