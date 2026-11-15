use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use inference_core::{ActiveInference, InferenceRequest, InitInference};

use crate::types::{InferenceCleanupFinished, InferenceLaunchError, InferenceLaunchOutput};

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
    // A launch result only closes the handoff; ActiveInference follows the real task.
    for result in world.event_reader::<Result<InferenceLaunchOutput, InferenceLaunchError>>() {
        if let Err(failure) = result {
            if let Some(fallback) = &failure.fallback {
                fallback.fail(failure.error.clone());
            }
        }
    }
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
    if !cleanup.source.load(std::sync::atomic::Ordering::Acquire) {
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
    if let Some(slot) = world.get_component::<lua_runtime_core::AgentWaitSlot>(cleanup.agent) {
        let _ = slot.0.lock().unwrap_or_else(|e| e.into_inner()).wake();
    }
}
