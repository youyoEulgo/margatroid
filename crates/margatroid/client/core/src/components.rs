use crate::{ClientIdentity, ConnectionId};
use core_plugin::Component;
use std::sync::Arc;
#[derive(Clone, Debug)]
pub struct Client {
    pub connection_id: ConnectionId,
    pub identity: Arc<ClientIdentity>,
}
impl std::ops::Deref for Client {
    type Target = ClientIdentity;
    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}
impl Component for Client {}
