use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerOptions {
    pub(crate) bind_address: SocketAddr,
    pub(crate) request_timeout: Duration,
    pub(crate) max_body_size: usize,
    pub(crate) shutdown_timeout: Duration,
}

impl HttpServerOptions {
    pub const DEFAULT_PORT: u16 = 3000;

    pub fn bind(address: SocketAddr) -> Self {
        Self {
            bind_address: address,
            ..Self::default()
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        assert!(!timeout.is_zero(), "request timeout must be positive");
        self.request_timeout = timeout;
        self
    }

    pub fn with_max_body_size(mut self, bytes: usize) -> Self {
        assert!(bytes > 0, "max body size must be greater than zero");
        self.max_body_size = bytes;
        self
    }

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        assert!(!timeout.is_zero(), "shutdown timeout must be positive");
        self.shutdown_timeout = timeout;
        self
    }

    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn max_body_size(&self) -> usize {
        self.max_body_size
    }

    pub fn shutdown_timeout(&self) -> Duration {
        self.shutdown_timeout
    }
}

impl Default for HttpServerOptions {
    fn default() -> Self {
        Self {
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), Self::DEFAULT_PORT),
            request_timeout: Duration::from_secs(30),
            max_body_size: 8 * 1024 * 1024,
            shutdown_timeout: Duration::from_secs(10),
        }
    }
}
