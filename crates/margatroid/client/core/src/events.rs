use crate::{ClientError, RegisteredClient};
use crate::{ClientSender, ConnectionId};
use core_plugin::Event;

pub type ClientRegistrationReply = std::sync::Arc<
    std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<RegisteredClient, ClientError>>>>,
>;

#[derive(Clone, Debug)]
pub struct RegisterClient {
    pub id: String,
    pub connection_id: ConnectionId,
    pub client_type: String,
    pub name: Option<String>,
    pub sender: ClientSender,
    pub reply: ClientRegistrationReply,
}
impl Event for RegisterClient {}
#[derive(Clone, Debug)]
pub struct ClientDisconnected {
    pub connection_id: ConnectionId,
}
impl Event for ClientDisconnected {}
