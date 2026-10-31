mod components;
mod error;
mod events;
mod types;

pub use components::Client;
pub use error::ClientError;
pub use events::ClientRegistrationResult;
pub use types::RegisteredClient;

use core_plugin::{Entity, World};
use server_plugin::{WebSocketConnectionId, WebSocketConnections, WebSocketSender};
use std::collections::HashSet;

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
        let mut ids = HashSet::new();
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
