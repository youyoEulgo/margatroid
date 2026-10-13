mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource};
use resource_id_plugin::{ResourcePluginInstalled, WorldResourceExt};
use server_plugin::WebSocketConnections;

pub use handler::registered_client;
pub use margatroid_types::client_components::{Client, WorldClientExt};
pub use margatroid_types::client_events::{
    ClientError, ClientRegistrationResult, RegisteredClient,
};
pub use types::client_source;

use crate::system::{client_disconnect_system, client_registration_system};

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
        if app.world().contains_resource::<ClientPluginInstalled>() {
            panic!("ClientPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("ClientPlugin schedule does not exist: {}", self.schedule);
        }
        if !app.world().contains_resource::<WebSocketConnections>() {
            panic!("ServerPlugin must be installed before ClientPlugin");
        }
        if !app.world().contains_resource::<ResourcePluginInstalled>() {
            panic!("ResourcePlugin must be installed before ClientPlugin");
        }
        app.world_mut()
            .register_library::<Client>("client")
            .expect("client library registration failed");
        app.world_mut().insert_resource(ClientPluginInstalled);
        app.add_system(&self.schedule, client_registration_system)
            .add_system(&self.schedule, client_disconnect_system);
    }
}
