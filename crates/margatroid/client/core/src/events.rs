use crate::{ClientError, RegisteredClient};
use crate::{ClientSender, ConnectionId};
use core_plugin::Event;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRegistrationResult {
    pub id: String,
    pub connection_id: ConnectionId,
    pub result: Result<RegisteredClient, ClientError>,
}

impl Event for ClientRegistrationResult {}

#[derive(Clone, Debug)]
pub struct RegisterClient {
    pub id: String,
    pub connection_id: ConnectionId,
    pub client_type: String,
    pub name: Option<String>,
    pub sender: ClientSender,
}
impl Event for RegisterClient {}
#[derive(Clone, Debug)]
pub struct ClientDisconnected {
    pub connection_id: ConnectionId,
}
impl Event for ClientDisconnected {}
