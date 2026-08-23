mod error;
mod events;
mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::WorldAsyncExt;
use config_plugin::MargatroidConfig;
pub use config_plugin::WebSocketMessageTarget;
use core_plugin::{App, Plugin, Resource};
use log_plugin::TracingStream;
use server_plugin::{AppServerExt, WebSocketConnections};

pub use events::*;

use crate::system::{collect_external_events_system, dto_route_system};
use crate::types::{BackendStateReportCache, PendingMclCommandResponses};

#[derive(Clone, Debug)]
pub struct DtoPlugin {
    websocket_path: String,
    schedule: String,
}

impl DtoPlugin {
    pub fn new() -> Self {
        Self {
            websocket_path: "/ws".into(),
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }

    pub fn with_websocket_path(mut self, path: impl Into<String>) -> Self {
        self.websocket_path = path.into();
        self
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for DtoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

struct DtoPluginInstalled;

impl Resource for DtoPluginInstalled {}

impl Plugin for DtoPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<DtoPluginInstalled>() {
            panic!("DtoPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("DtoPlugin schedule does not exist: {}", self.schedule);
        }
        if !app.world().contains_resource::<WebSocketConnections>() {
            panic!("ServerPlugin must be installed before DtoPlugin");
        }
        let targets = app
            .world()
            .get_resource::<MargatroidConfig>()
            .cloned()
            .expect("ConfigPlugin must be installed before DtoPlugin");
        let stream = app
            .world()
            .get_resource::<TracingStream>()
            .cloned()
            .expect("LogPlugin with a TracingStream must be installed before DtoPlugin");
        let events = app.world().event_sender();
        app.world().spawn_async_service(handler::forward_logs(
            stream,
            events,
            targets.logs().to_vec(),
        ));
        app.world_mut().insert_resource(DtoPluginInstalled);
        app.world_mut()
            .insert_resource(BackendStateReportCache::default());
        app.world_mut()
            .insert_resource(PendingMclCommandResponses::default());
        app.add_websocket_event_route(&self.websocket_path)
            .add_system(&self.schedule, collect_external_events_system)
            .add_system(&self.schedule, dto_route_system);
    }
}
