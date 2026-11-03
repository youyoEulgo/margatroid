use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use inference_core::{ActiveInference, InferenceRequest, InitInference};
use lua_runtime_core::{DriverLuaVm, WorldLuaRuntimeExt};

use crate::types::InferenceCleanupFinished;

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
    let cleanups = world
        .event_reader::<InferenceCleanupFinished>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for cleanup in cleanups {
        handle_cleanup_finished(world, cleanup);
    }
    let requests = world
        .event_reader::<InferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_inference(world, request);
    }
}

pub(crate) fn handle_cleanup_finished(world: &mut World, cleanup: InferenceCleanupFinished) {
    if cleanup.instance.owner != cleanup.agent
        || !world
            .get_component::<DriverLuaVm>(cleanup.agent)
            .is_some_and(|vm| vm.instance == cleanup.instance)
    {
        return;
    }
    let Some(handle) = world
        .get_component::<ActiveInference>(cleanup.agent)
        .and_then(|active| active.current.as_ref())
        .filter(|handle| handle.request_id == cleanup.request_id)
    else {
        return;
    };
    if !handle.is_finished() {
        // The event may arrive before Tokio marks the supervisor task complete.
        // Keep the wake pending until finish_turn can observe the real join.
        world.send_event_after(cleanup, 1);
        return;
    }
    let _ = world.driver_wake(&cleanup.instance);
}
