mod components;
mod error;
mod events;
mod output;
mod shared;
mod types;

pub use components::Client;
pub use error::{ClientError, ClientSendError};
pub use events::{ClientDisconnected, ClientRegistrationReply, RegisterClient};
pub use output::{
    ClientPrepareError, ClientReceiver, ClientSender, PrepareFuture, PreparedState, StatePrepare,
    SyncContext, SyncKey, SyncSource,
};
pub use shared::{ClientIdentity, ClientSelector, Clients, RequestTicket};
pub use types::{ClientFrame, ConnectionId, RegisteredClient};

use core_plugin::{Entity, World};

pub trait WorldClientExt {
    fn client_by_connection(&self, connection: ConnectionId) -> Option<(Entity, &Client)>;
    fn client_webui_senders(&self) -> Vec<ClientSender>;
    fn clients(&self) -> Clients;
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

    fn clients(&self) -> Clients {
        self.get_resource::<Clients>()
            .expect("Clients resource missing")
            .clone()
    }
    fn client_webui_senders(&self) -> Vec<ClientSender> {
        self.clients()
            .select(&[ClientSelector::Type("webui".into())])
            .into_iter()
            .map(|(_, sender)| sender)
            .collect()
    }
}

mod probe;
pub use probe::UpdateProbe;

mod probe_factory;
pub use probe_factory::ProbeFactory;
