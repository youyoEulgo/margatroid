use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStartRequested;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStarted {
    pub address: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFailed {
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownRequested;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequestReceived {
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPromptSubmitted {
    pub workspace: String,
    pub prompt: String,
}
