use core_plugin::Event;
use resource_id_plugin::ResourceRef;
use server_plugin::WebSocketConnectionId;

use crate::{Client, ClientError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredClient {
    pub resource_id: ResourceRef,
    pub client: Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRegistrationResult {
    pub id: String,
    pub connection_id: WebSocketConnectionId,
    pub result: Result<RegisteredClient, ClientError>,
}

impl Event for ClientRegistrationResult {}
