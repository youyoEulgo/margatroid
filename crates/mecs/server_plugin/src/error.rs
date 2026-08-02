use std::fmt;
use std::time::Duration;

use axum::http::Method;

use crate::websocket::WebSocketConnectionId;

#[non_exhaustive]
#[derive(Debug)]
pub enum ServerError {
    RuntimePluginMissing,
    AsyncRuntimePluginMissing,
    ServerPluginAlreadyInstalled,
    ServerPluginMissing,
    RoutesFrozen,
    EventRouteAlreadyRegistered { method: Method, path: String },
    WebSocketRouteAlreadyRegistered { path: String },
    UnsupportedMethod { method: Method },
    InvalidBodyLimit { limit: usize },
    InvalidResponseStartTimeout { timeout: Duration },
    InvalidStreamBufferCapacity { capacity: usize },
    InvalidWebSocketBufferCapacity { capacity: usize },
    InvalidShutdownTimeout { timeout: Duration },
}

impl ServerError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimePluginMissing => formatter.write_str("RuntimePlugin is not installed"),
            Self::AsyncRuntimePluginMissing => {
                formatter.write_str("AsyncRuntimePlugin is not installed")
            }
            Self::ServerPluginAlreadyInstalled => {
                formatter.write_str("ServerPlugin is already installed")
            }
            Self::ServerPluginMissing => formatter.write_str("ServerPlugin is not installed"),
            Self::RoutesFrozen => formatter.write_str("server routes are already frozen"),
            Self::EventRouteAlreadyRegistered { method, path } => {
                write!(
                    formatter,
                    "event route `{method} {path}` is already registered"
                )
            }
            Self::WebSocketRouteAlreadyRegistered { path } => {
                write!(formatter, "WebSocket route `{path}` is already registered")
            }
            Self::UnsupportedMethod { method } => {
                write!(
                    formatter,
                    "HTTP method `{method}` is not supported by Axum routing"
                )
            }
            Self::InvalidBodyLimit { limit } => {
                write!(
                    formatter,
                    "request body limit must be positive, got {limit}"
                )
            }
            Self::InvalidResponseStartTimeout { timeout } => write!(
                formatter,
                "response start timeout must be positive, got {timeout:?}"
            ),
            Self::InvalidStreamBufferCapacity { capacity } => write!(
                formatter,
                "HTTP stream buffer capacity must be positive, got {capacity}"
            ),
            Self::InvalidWebSocketBufferCapacity { capacity } => write!(
                formatter,
                "WebSocket buffer capacity must be positive, got {capacity}"
            ),
            Self::InvalidShutdownTimeout { timeout } => write!(
                formatter,
                "server shutdown timeout must be positive, got {timeout:?}"
            ),
        }
    }
}

impl std::error::Error for ServerError {}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpResponseError {
    ResponseAlreadyStarted,
    StreamNotStarted,
    ResponseClosed,
    RequestClosed,
}

impl fmt::Display for HttpResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ResponseAlreadyStarted => "HTTP response has already started",
            Self::StreamNotStarted => "HTTP response stream has not started",
            Self::ResponseClosed => "HTTP response is closed",
            Self::RequestClosed => "HTTP request is closed",
        })
    }
}

impl std::error::Error for HttpResponseError {}

#[derive(Debug)]
pub struct HttpStreamError {
    message: String,
}

impl HttpStreamError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn abandoned() -> Self {
        Self::new("HTTP response stream was abandoned without finish or abort")
    }
}

impl fmt::Display for HttpStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpStreamError {}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketSendError {
    BufferFull,
    ConnectionClosed,
}

impl fmt::Display for WebSocketSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BufferFull => "WebSocket send buffer is full",
            Self::ConnectionClosed => "WebSocket connection is closed",
        })
    }
}

impl std::error::Error for WebSocketSendError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum WebSocketNameError {
    ConnectionNotFound {
        connection_id: WebSocketConnectionId,
    },
    NameAlreadyExists {
        name: String,
    },
}

impl fmt::Display for WebSocketNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionNotFound { connection_id } => write!(
                formatter,
                "WebSocket connection {} does not exist",
                connection_id.get()
            ),
            Self::NameAlreadyExists { name } => {
                write!(
                    formatter,
                    "WebSocket connection name `{name}` already exists"
                )
            }
        }
    }
}

impl std::error::Error for WebSocketNameError {}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketStreamReceiveError {
    Aborted,
    ConnectionClosed,
}

impl fmt::Display for WebSocketStreamReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Aborted => "WebSocket stream was aborted",
            Self::ConnectionClosed => "WebSocket connection closed before the stream finished",
        })
    }
}

impl std::error::Error for WebSocketStreamReceiveError {}
