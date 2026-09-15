mod error;
mod events;
mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Component, Plugin, Resource};
use resource_id_plugin::ResourceIdPluginInstalled;
use server_plugin::{WebSocketConnectionId, WebSocketConnections};

pub use error::ClientError;
pub use types::client_source;

use crate::system::{client_disconnect_system, client_registration_system};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    connection_id: WebSocketConnectionId,
    client_type: String,
    name: String,
}

impl Client {
    pub(crate) fn new(
        connection_id: WebSocketConnectionId,
        client_type: String,
        name: String,
    ) -> Self {
        Self {
            connection_id,
            client_type,
            name,
        }
    }

    pub fn connection_id(&self) -> WebSocketConnectionId {
        self.connection_id
    }

    pub fn client_type(&self) -> &str {
        &self.client_type
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Component for Client {}

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
        if !app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("ResourceIdPlugin must be installed before ClientPlugin");
        }
        app.world_mut().insert_resource(ClientPluginInstalled);
        app.add_system(&self.schedule, client_registration_system)
            .add_system(&self.schedule, client_disconnect_system);
    }
}
