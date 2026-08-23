mod error;
mod events;
mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource};
use server_plugin::WebSocketConnections;

use crate::system::connection_registration_system;

#[derive(Clone, Debug)]
pub struct ConnectionPlugin {
    schedule: String,
}

impl ConnectionPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for ConnectionPlugin {
    fn default() -> Self {
        Self::new()
    }
}

struct ConnectionPluginInstalled;

impl Resource for ConnectionPluginInstalled {}

impl Plugin for ConnectionPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<ConnectionPluginInstalled>() {
            panic!("ConnectionPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!(
                "ConnectionPlugin schedule does not exist: {}",
                self.schedule
            );
        }
        if !app.world().contains_resource::<WebSocketConnections>() {
            panic!("ServerPlugin must be installed before ConnectionPlugin");
        }
        app.world_mut().insert_resource(ConnectionPluginInstalled);
        app.add_system(&self.schedule, connection_registration_system);
    }
}
