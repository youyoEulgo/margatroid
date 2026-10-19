mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Entity, Plugin, Resource, World};
use resource_id_plugin::{ResourcePluginInstalled, WorldResourceExt};
use server_plugin::{WebSocketConnectionId, WebSocketConnections, WebSocketSender};

pub use handler::registered_client;
pub use margatroid_types::client_components::Client;
pub use margatroid_types::client_events::{ClientError, ClientRegistrationResult};
pub use margatroid_types::client_types::{client_source, RegisteredClient};

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

pub trait WorldClientExt {
    fn client_by_connection(&self, connection: WebSocketConnectionId) -> Option<(Entity, &Client)>;
    fn client_webui_senders(&self) -> Vec<WebSocketSender>;
}
impl WorldClientExt for World {
    fn client_by_connection(&self, connection: WebSocketConnectionId) -> Option<(Entity, &Client)> {
        self.query_with::<Client>()
            .result()
            .into_iter()
            .find_map(|entity| {
                let client = self.get_component::<Client>(entity)?;
                (client.connection_id == connection).then_some((entity, client))
            })
    }
    fn client_webui_senders(&self) -> Vec<WebSocketSender> {
        let Some(connections) = self.get_resource::<WebSocketConnections>() else {
            return Vec::new();
        };
        let mut ids = std::collections::HashSet::new();
        self.query_with::<Client>()
            .result()
            .into_iter()
            .filter_map(|entity| {
                let client = self.get_component::<Client>(entity)?;
                if client.client_type != "webui" || !ids.insert(client.connection_id) {
                    return None;
                }
                connections
                    .get(client.connection_id)
                    .filter(|sender| !sender.is_closed())
            })
            .collect()
    }
}
