use crate::{ClientError, RegisteredClient};
use core_plugin::Event;
use server_plugin::WebSocketConnectionId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRegistrationResult {
    pub id: String,
    pub connection_id: WebSocketConnectionId,
    pub result: Result<RegisteredClient, ClientError>,
}

impl Event for ClientRegistrationResult {}
