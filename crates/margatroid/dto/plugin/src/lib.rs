mod handler;
mod probes;
mod workspaces;
pub use probes::{agent_sync_context, model_sync_binding, resource_sync_bindings};
pub use workspaces::{WorkspaceSyncBinding, workspace_sync_binding};
mod monitor;
mod projection;
mod records;
mod system;
mod types;
mod ui;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncContext, AsyncRuntimeHandle, WorldAsyncExt};
use axum_plugin::{AppAxumExt, AxumHandle};
use config_core::MargatroidConfig;
use core_plugin::{App, Plugin};
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
        if !app.world().contains_resource::<WorkspaceSyncBinding>() {
            let binding = workspace_sync_binding(
                app.world()
                    .get_resource::<client_core::Clients>()
                    .unwrap()
                    .clone(),
                app.world()
                    .get_resource::<MargatroidConfig>()
                    .unwrap()
                    .backend_state(),
            );
            app.world_mut().insert_resource(binding.updates.clone());
            app.world_mut().insert_resource(binding);
        }
        let agent_context = probes::get_or_create_agent_context(app.world_mut());
        probes::install_resource_bindings(app.world_mut(), agent_context);
        let state = DtoState {
            ingress: app
                .world()
                .get_resource::<MargatroidConfig>()
                .expect("configuration required")
                .server_ingress()
                .clone(),
            events: Default::default(),
            clients: app
                .world()
                .get_resource::<client_core::Clients>()
                .expect("ClientPlugin required")
                .clone(),
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
        app.world_mut().insert_resource(state.clone());
        app.world_mut().insert_resource(SyncObservations::default());
        probes::initialize_agent_sources(app.world_mut());
        workspaces::install_workspace_monitor(app.world_mut());
        app.add_axum_routes(
            axum::Router::new()
                .route("/ws", axum::routing::get(handler::ws_handler))
                .with_state(state),
        )
        .add_system(RuntimePlugin::PRE_UPDATE, system::abort_control_system)
        .add_system(RuntimePlugin::UPDATE, system::dto_route_system);
    }
}

pub fn task_sync_binding(
    clients: client_core::Clients,
    targets: &[config_core::OutboundTarget],
) -> tool_core::TaskSyncBinding {
    use std::sync::Arc;
    let targets = targets
        .iter()
        .map(|target| match target {
            config_core::OutboundTarget::Broadcast => client_core::ClientSelector::Broadcast,
            config_core::OutboundTarget::Type(t) => client_core::ClientSelector::Type(t.clone()),
            config_core::OutboundTarget::Name(n) => client_core::ClientSelector::Name(n.clone()),
        })
        .collect::<Vec<_>>();
    tool_core::TaskSyncBinding::new(clients, targets.into(), |snapshot| {
        let tasks = snapshot
            .tasks
            .into_iter()
            .map(|task| margatroid_protocol::AgentTaskDto {
                task_id: task.task_id,
                call_id: task.call_id,
                alias: task.alias,
                returned: task.returned,
                output: task.output,
                error: task.error.map(|error| {
                    margatroid_protocol::ProtocolErrorDto::new(
                        format!("{:?}", error.kind),
                        error.message,
                    )
                }),
            })
            .collect();
        let message = margatroid_protocol::ServerMessage::AgentTaskSync {
            address: snapshot.address,
            tasks,
        };
        margatroid_protocol::encode_server_message(&message)
            .map(|text| Arc::new(client_core::ClientFrame::Text(text)))
            .map_err(|_| client_core::ClientPrepareError::TooLarge)
    })
}

pub(crate) fn sync_selectors(
    targets: &[config_core::OutboundTarget],
) -> std::sync::Arc<[client_core::ClientSelector]> {
    targets
        .iter()
        .map(|target| match target {
            config_core::OutboundTarget::Broadcast => client_core::ClientSelector::Broadcast,
            config_core::OutboundTarget::Type(t) => client_core::ClientSelector::Type(t.clone()),
            config_core::OutboundTarget::Name(n) => client_core::ClientSelector::Name(n.clone()),
        })
        .collect::<Vec<_>>()
        .into()
}
