use crate::handler::{handle_inference, handle_model_routes_reloaded, handle_reload_model_routes};
use core_plugin::World;
use margatroid_types::inference_events::{
    InferenceReloadOutput as ReloadOutput, InferenceRequest, ReloadModelRoutes,
};

pub(crate) fn inference_system(world: &mut World) {
    let outputs = world
        .event_reader::<ReloadOutput>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for output in outputs {
        handle_model_routes_reloaded(world, output);
    }
    let reloads = world
        .event_reader::<ReloadModelRoutes>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in reloads {
        handle_reload_model_routes(world, request);
    }
    let requests = world
        .event_reader::<InferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_inference(world, request);
    }
}
