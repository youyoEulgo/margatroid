mod handler;
mod probes;
mod workspaces;
pub use probes::{
    ToolObserverFactory, agent_sync_context, model_sync_binding, resource_sync_bindings,
    sandbox_sync_binding,
};
pub use workspaces::{WorkspaceSyncBinding, workspace_sync_binding};
mod monitor;
mod projection;
mod records;
mod settings;
mod system;
mod types;
mod ui;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncContext, AsyncRuntimeHandle, WorldAsyncExt};
use axum_plugin::{AppAxumExt, AxumHandle};
use closure_plugin::WorldClosureExt;
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
            let bindings = sync_bindings(
                app.world()
                    .get_resource::<client_core::Clients>()
                    .unwrap()
                    .clone(),
            );
            app.world_mut().insert_resource(bindings.workspace_binding);
            app.world_mut().insert_resource(bindings.workspace);
            if !app
                .world()
                .contains_resource::<agent_core::AgentUpdateContext>()
            {
                app.world_mut().insert_resource(bindings.agent);
            }
            app.world_mut()
                .insert_resource(TaskSyncInstaller(bindings.tasks));
        }
        let agent_context = probes::get_or_create_agent_context(app.world_mut());
        probes::install_resource_bindings(app.world_mut(), agent_context);
        let state = DtoState {
            world: app.world().closure_sender(RuntimePlugin::UPDATE),
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
        probes::install_model_config_monitor(app.world_mut());
        app.add_axum_routes(
            axum::Router::new()
                .route("/ws", axum::routing::get(handler::ws_handler))
                .route(
                    "/{workspace}/{*path}",
                    axum::routing::get(handler::workspace_file_handler),
                )
                .with_state(state),
        )
        .add_system(RuntimePlugin::PRE_UPDATE, system::abort_control_system)
        .add_system(RuntimePlugin::UPDATE, system::dto_route_system);
    }
}

pub type TaskObserverBinder =
    std::sync::Arc<dyn Fn(core_plugin::Entity, String, &tool_core::ToolTasks) + Send + Sync>;
#[derive(Clone)]
pub struct TaskSyncInstaller(pub TaskObserverBinder);
impl core_plugin::Resource for TaskSyncInstaller {}
pub struct DtoSyncBindings {
    pub agent: agent_core::AgentUpdateContext,
    pub workspace: workspace_core::WorkspaceUpdateContext,
    pub workspace_binding: WorkspaceSyncBinding,
    pub tasks: TaskObserverBinder,
    pub tools: ToolObserverFactory,
    pub models: inference_core::ModelObserverFactory,
    pub sandboxes: sandbox_resource_core::SandboxObserverFactory,
}
pub fn sync_bindings(clients: client_core::Clients) -> DtoSyncBindings {
    let agent = agent_sync_context(clients.clone());
    let workspace_binding = workspace_sync_binding(clients.clone());
    DtoSyncBindings {
        workspace: workspace_binding.updates.clone(),
        tools: resource_sync_bindings(agent.clone()),
        models: model_sync_binding(agent.clone()),
        sandboxes: sandbox_sync_binding(agent.clone()),
        agent,
        workspace_binding,
        tasks: task_sync_binding(clients),
    }
}
pub fn task_sync_binding(clients: client_core::Clients) -> TaskObserverBinder {
    use std::sync::{Arc, Mutex};
    let sources = Arc::new(Mutex::new(std::collections::BTreeMap::<
        core_plugin::Entity,
        (
            tool_core::TaskUpdateContext,
            client_core::SyncContext<tool_core::AgentTaskSnapshot>,
        ),
    >::new()));
    Arc::new(move |owner, address, tasks| {
        let mut all = sources.lock().unwrap_or_else(|e| e.into_inner());
        if all
            .get(&owner)
            .is_some_and(|(collection, _)| collection.same_collection(&tasks.update_context()))
        {
            return;
        }
        if let Some((_, previous)) = all.remove(&owner) {
            previous.close();
        }
        let source = tasks.snapshot_source();
        let target = address.clone();
        let sync = client_core::SyncContext::new(
            clients.clone(),
            client_core::SyncKey {
                topic: "agent.task.sync".into(),
                object: address,
            },
            sync_selectors(),
            move || {
                let (version, tasks) =
                    source().ok_or(client_core::ClientPrepareError::SourceClosed)?;
                Ok((
                    version,
                    tool_core::AgentTaskSnapshot {
                        address: target.clone(),
                        tasks,
                    },
                ))
            },
            |snapshot: tool_core::AgentTaskSnapshot| {
                let tasks = snapshot
                    .tasks
                    .into_iter()
                    .map(|t| margatroid_protocol::AgentTaskDto {
                        task_id: t.task_id,
                        call_id: t.call_id,
                        alias: t.alias,
                        returned: t.returned,
                        output: t.output,
                        error: t.error.map(|e| {
                            margatroid_protocol::ProtocolErrorDto::new(
                                format!("{:?}", e.kind),
                                e.message,
                            )
                        }),
                    })
                    .collect();
                margatroid_protocol::encode_server_message(
                    &margatroid_protocol::ServerMessage::AgentTaskSync {
                        address: snapshot.address,
                        tasks,
                    },
                )
                .map(|text| Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::TooLarge)
            },
        );
        let changed = sync.clone();
        let retired = sync.clone();
        let weak = Arc::downgrade(&sources);
        let identity = tasks.update_context();
        tasks.observe(
            Arc::new(move |version| changed.publish(version)),
            Box::new(move || {
                retired.close();
                if let Some(sources) = weak.upgrade() {
                    let mut sources = sources.lock().unwrap_or_else(|e| e.into_inner());
                    if sources
                        .get(&owner)
                        .is_some_and(|(collection, _)| collection.same_collection(&identity))
                    {
                        sources.remove(&owner);
                    }
                }
            }),
        );
        all.insert(owner, (tasks.update_context(), sync));
    })
}

pub(crate) fn sync_selectors() -> std::sync::Arc<[client_core::ClientSelector]> {
    vec![client_core::ClientSelector::Type("webui".into())].into()
}
