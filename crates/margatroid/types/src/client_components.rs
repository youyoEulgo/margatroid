use core_plugin::{Component, Entity, World};
use resource_id_plugin::{LibraryComponent, ResourceID};
use server_plugin::{WebSocketConnectionId, WebSocketConnections, WebSocketSender};

#[derive(Clone, Debug)]
pub struct Client {
    pub resource_id: ResourceID,
    pub connection_id: WebSocketConnectionId,
    pub client_type: String,
    pub name: String,
}
impl Component for Client {}
impl LibraryComponent for Client {
    fn resource_id(&self) -> &ResourceID {
        &self.resource_id
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
