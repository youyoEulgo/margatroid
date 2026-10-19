use core_plugin::Component;
use resource_id_plugin::{LibraryComponent, ResourceID};
use server_plugin::WebSocketConnectionId;

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
