use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use core_plugin::Resource;

use crate::ServerError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerOptions {
    pub(crate) bind_address: SocketAddr,
    pub(crate) body_limit: usize,
    pub(crate) response_start_timeout: Duration,
    pub(crate) stream_buffer_capacity: usize,
    pub(crate) websocket_buffer_capacity: usize,
    pub(crate) shutdown_timeout: Duration,
}

impl ServerOptions {
    pub const DEFAULT_PORT: u16 = 3939;

    pub fn bind(address: SocketAddr) -> Self {
        Self {
            bind_address: address,
            ..Self::default()
        }
    }

    pub fn with_bind(mut self, address: SocketAddr) -> Self {
        self.bind_address = address;
        self
    }

    pub fn with_body_limit(mut self, limit: usize) -> Self {
        if limit == 0 {
            ServerError::InvalidBodyLimit { limit }.panic();
        }
        self.body_limit = limit;
        self
    }

    pub fn with_response_start_timeout(mut self, timeout: Duration) -> Self {
        if timeout.is_zero() {
            ServerError::InvalidResponseStartTimeout { timeout }.panic();
        }
        self.response_start_timeout = timeout;
        self
    }

    pub fn with_stream_buffer_capacity(mut self, capacity: usize) -> Self {
        if capacity == 0 {
            ServerError::InvalidStreamBufferCapacity { capacity }.panic();
        }
        self.stream_buffer_capacity = capacity;
        self
    }

    pub fn with_websocket_buffer_capacity(mut self, capacity: usize) -> Self {
        if capacity == 0 {
            ServerError::InvalidWebSocketBufferCapacity { capacity }.panic();
        }
        self.websocket_buffer_capacity = capacity;
        self
    }

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        if timeout.is_zero() {
            ServerError::InvalidShutdownTimeout { timeout }.panic();
        }
        self.shutdown_timeout = timeout;
        self
    }

    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    pub fn response_start_timeout(&self) -> Duration {
        self.response_start_timeout
    }

    pub fn stream_buffer_capacity(&self) -> usize {
        self.stream_buffer_capacity
    }

    pub fn websocket_buffer_capacity(&self) -> usize {
        self.websocket_buffer_capacity
    }

    pub fn shutdown_timeout(&self) -> Duration {
        self.shutdown_timeout
    }
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), Self::DEFAULT_PORT),
            body_limit: 8 * 1024 * 1024,
            response_start_timeout: Duration::from_secs(30),
            stream_buffer_capacity: 32,
            websocket_buffer_capacity: 64,
            shutdown_timeout: Duration::from_secs(10),
        }
    }
}

impl Resource for ServerOptions {}
