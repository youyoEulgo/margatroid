use core_plugin::Event;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WebSocketMessageTarget {
    Broadcast,
    Type(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebSocketMessageSend {
    pub target: WebSocketMessageTarget,
    pub message: serde_json::Value,
}
impl Event for WebSocketMessageSend {}
