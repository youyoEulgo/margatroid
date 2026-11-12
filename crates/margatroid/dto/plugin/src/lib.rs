mod handler;
mod projection;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncContext, AsyncRuntimeHandle, WorldAsyncExt};
use axum_plugin::{AppAxumExt, AxumHandle};
use config_core::MargatroidConfig;
use core_plugin::{App, Plugin};
pub use types::WebSocketMessageSend;
use types::*;

#[derive(Default)]
pub struct DtoPlugin;
impl DtoPlugin {
    pub fn new() -> Self {
        Self
    }
}
impl Plugin for DtoPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>()
        );
        assert!(!app.world().contains_resource::<DtoState>());
        let lifetime = app
            .world()
            .get_resource::<AxumHandle>()
            .expect("AxumPlugin required")
            .lifetime();
        let state = DtoState {
            ingress: app
                .world()
                .get_resource::<MargatroidConfig>()
                .expect("configuration required")
                .server_ingress()
                .clone(),
            events: Default::default(),
            connections: Default::default(),
            lifetime,
            definitions: app
                .world()
                .get_resource::<workspace_core::WorkspaceDefinitions>()
                .cloned()
                .unwrap_or_default(),
        };
        let events = state.events.clone();
        app.world()
            .send_async_closure(RuntimePlugin::UPDATE, move |context: AsyncContext| {
                let _ = events.set(context);
                async { Ok::<_, DtoAsyncError>(DtoInitialized) }
            });
        app.world_mut().insert_resource(state.connections.clone());
        app.world_mut().insert_resource(state.clone());
        app.world_mut().insert_resource(SyncState::default());
        app.add_axum_routes(
            axum::Router::new()
                .route("/ws", axum::routing::get(handler::ws_handler))
                .with_state(state),
        )
        .add_system(RuntimePlugin::PRE_UPDATE, system::abort_control_system)
        .add_system(RuntimePlugin::UPDATE, system::dto_route_system)
        .add_system(RuntimePlugin::UPDATE, system::dto_sync_system);
    }
}
