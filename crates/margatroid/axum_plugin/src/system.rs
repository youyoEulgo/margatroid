use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AsyncContext, WorldAsyncExt};
use core_plugin::World;

use crate::types::{AxumBind, AxumContext, AxumStarted, AxumStopped, AxumTaskError};
use crate::{AxumHandle, AxumRoutes};

pub(crate) fn start_axum_system(world: &mut World) {
    let router = world
        .get_resource_mut::<AxumRoutes>()
        .unwrap()
        .router
        .take()
        .unwrap();
    let handle = world.get_resource::<AxumHandle>().unwrap().clone();
    let mut context = AxumContext {
        router,
        bind: world.get_resource::<AxumBind>().unwrap().0,
        owner: Some(crate::handler::CompletionOwner(handle.clone())),
        handle,
        events: None,
    };
    world.send_async_closure(RuntimePlugin::UPDATE, move |events: AsyncContext| {
        context.events = Some(events);
        crate::handler::run_axum(context)
    });
}

pub(crate) fn axum_result_system(world: &mut World) {
    let handle = world.get_resource::<AxumHandle>().unwrap();
    for started in world.event_reader::<AxumStarted>() {
        *handle.address.write().unwrap() = Some(started.0);
    }
    for result in world.event_reader::<Result<AxumStopped, AxumTaskError>>() {
        match result {
            Ok(_) => handle.complete(Ok(())),
            Err(error) => {
                handle.request_shutdown();
                handle.complete(Err(error.0.clone()));
                tracing::error!(error = %error.0, "Axum service stopped with error");
            }
        }
    }
}
