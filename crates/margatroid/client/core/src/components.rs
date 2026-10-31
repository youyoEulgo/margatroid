use core_plugin::Component;
use server_plugin::WebSocketConnectionId;

#[derive(Clone, Debug)]
pub struct Client {
    pub connection_id: WebSocketConnectionId,
    pub client_type: String,
    pub name: String,
}

impl Component for Client {}
