mod handler;
mod system;

pub use client_core::{
    Client, ClientError, ClientRegistrationResult, RegisteredClient, WorldClientExt,
};

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource};
use server_plugin::WebSocketConnections;

#[derive(Clone, Debug)]
pub struct ClientPlugin {
    schedule: String,
}

impl ClientPlugin {
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

impl Default for ClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

struct ClientPluginInstalled;
impl Resource for ClientPluginInstalled {}

impl Plugin for ClientPlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<ClientPluginInstalled>(),
            "ClientPlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "ClientPlugin schedule does not exist: {}",
            self.schedule
        );
        assert!(
            app.world().contains_resource::<WebSocketConnections>(),
            "ServerPlugin must be installed before ClientPlugin"
        );
        app.world_mut().insert_resource(ClientPluginInstalled);
        app.add_system(&self.schedule, system::client_registration_system)
            .add_system(&self.schedule, system::client_disconnect_system);
    }
}
