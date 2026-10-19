use crate::client_types::RegisteredClient;
use core_plugin::Event;
use server_plugin::WebSocketConnectionId;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    InvalidRequest,
    InvalidClientType,
    InvalidResourceID,
    DuplicateName,
    DuplicateClient,
    ConnectionMissing,
    ResourceSetupFailed,
}
impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "client registration request ID is empty",
            Self::InvalidClientType => "client type is not a stable identifier",
            Self::InvalidResourceID => "client resource id could not be built",
            Self::DuplicateName => "client name is already in use",
            Self::DuplicateClient => "client is already registered",
            Self::ConnectionMissing => "connection disappeared before registration",
            Self::ResourceSetupFailed => "client resource setup failed",
        })
    }
}
impl std::error::Error for ClientError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRegistrationResult {
    pub id: String,
    pub connection_id: WebSocketConnectionId,
    pub result: Result<RegisteredClient, ClientError>,
}
impl Event for ClientRegistrationResult {}
