mod components;
mod error;
mod events;
mod types;

pub use components::Client;
pub use error::{ClientError, ClientSendError};
pub use events::{ClientDisconnected, ClientRegistrationReply, RegisterClient};
pub use types::{ClientFrame, ClientSender, ConnectionId, RegisteredClient};

use core_plugin::{Entity, World};
use std::collections::HashSet;

pub trait WorldClientExt {
    fn client_by_connection(&self, connection: ConnectionId) -> Option<(Entity, &Client)>;
    fn client_webui_senders(&self) -> Vec<ClientSender>;
}

impl WorldClientExt for World {
    fn client_by_connection(&self, connection: ConnectionId) -> Option<(Entity, &Client)> {
        self.query_with::<Client>()
            .result()
            .into_iter()
            .find_map(|entity| {
                let client = self.get_component::<Client>(entity)?;
                (client.connection_id == connection).then_some((entity, client))
            })
    }

    fn client_webui_senders(&self) -> Vec<ClientSender> {
        let mut ids = HashSet::new();
        self.query_with::<Client>()
            .result()
            .into_iter()
            .filter_map(|entity| {
                let client = self.get_component::<Client>(entity)?;
                if client.client_type != "webui" || !ids.insert(client.connection_id) {
                    return None;
                }
                (!client.sender.is_closed()).then(|| client.sender.clone())
            })
            .collect()
    }
}
