use config_plugin::WebSocketMessageTarget;
use core_plugin::Event;
use margatroid_protocol::ServerMessage;

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketMessageSend {
    pub target: WebSocketMessageTarget,
    pub message: ServerMessage,
}

impl Event for WebSocketMessageSend {}
