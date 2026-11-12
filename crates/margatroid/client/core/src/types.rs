use core_plugin::Entity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredClient {
    pub client: Entity,
    pub address: String,
    pub client_type: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);
impl ConnectionId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientFrame {
    Text(String),
    Binary(Vec<u8>),
    Close { code: u16, reason: String },
}
#[derive(Clone, Debug)]
pub struct ClientSender {
    sender: tokio::sync::mpsc::Sender<ClientFrame>,
}
impl ClientSender {
    pub fn new(sender: tokio::sync::mpsc::Sender<ClientFrame>) -> Self {
        Self { sender }
    }
    pub fn try_send(&self, frame: ClientFrame) -> Result<(), crate::ClientSendError> {
        self.sender.try_send(frame).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => crate::ClientSendError::Full,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => crate::ClientSendError::Closed,
        })
    }
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}
