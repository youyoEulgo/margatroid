use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::World;
use inference_core::{ActiveInference, InferenceRequest, InitInference};

use crate::types::{
    HttpRuntime, InferenceCleanupFinished, InferenceLaunchError, InferenceLaunchOutput,
};

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

fn wake_current(world: &mut World, cleanup: InferenceCleanupFinished) {
    if !cleanup.source.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some(agent) = world
        .get_component::<agent_core::Agent>(cleanup.agent)
        .filter(|agent| {
            agent.instance() == cleanup.instance && agent.state() != agent_core::AgentState::Closing
        })
    else {
        return;
    };
    let Some(handle) = world
        .get_component::<ActiveInference>(cleanup.agent)
        .and_then(|active| active.current.as_ref())
        .filter(|handle| {
            handle.request_id == cleanup.request_id
                && std::sync::Arc::ptr_eq(&handle.source_lease(), &cleanup.source)
        })
    else {
        return;
    };
    if handle.is_finished() {
        let _ = agent
            .wake_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wake();
    }
}

pub(crate) fn handle_cleanup_finished(world: &mut World, cleanup: InferenceCleanupFinished) {
    if !cleanup.source.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some(handle) = world
        .get_component::<ActiveInference>(cleanup.agent)
        .and_then(|active| active.current.as_ref())
        .filter(|handle| {
            handle.request_id == cleanup.request_id
                && std::sync::Arc::ptr_eq(&handle.source_lease(), &cleanup.source)
        })
        .cloned()
    else {
        return;
    };
    if handle.is_finished() {
        wake_current(world, cleanup);
        return;
    }
    let schedule = world
        .get_resource::<HttpRuntime>()
        .expect("inference runtime missing")
        .schedule
        .clone();
    let sender = world.closure_sender(&schedule);
    world
        .get_resource::<AsyncRuntimeHandle>()
        .expect("inference async runtime missing")
        .spawn(async move {
            if handle.wait_finished().await.is_ok() {
                sender.send(move |world| wake_current(world, cleanup));
            }
        });
}
