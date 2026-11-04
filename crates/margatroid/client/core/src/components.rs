use crate::{ClientSender, ConnectionId};
use core_plugin::Component;

#[derive(Clone, Debug)]
pub struct Client {
    pub connection_id: ConnectionId,
    pub client_type: String,
    pub name: String,
    pub sender: ClientSender,
}

impl Component for Client {}
