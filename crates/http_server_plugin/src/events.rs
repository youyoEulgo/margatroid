use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerStarted {
    pub address: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerFailed {
    pub message: String,
}
