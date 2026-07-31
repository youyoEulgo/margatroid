use std::any::Any;
use std::fmt;
use std::io;

use tokio::task::JoinError;

#[non_exhaustive]
#[derive(Debug)]
pub enum AsyncTaskError {
    Panicked { message: String },
    Cancelled,
}

impl AsyncTaskError {
    pub(crate) fn from_join_error(error: JoinError) -> Self {
        if error.is_panic() {
            Self::Panicked {
                message: panic_message(error.into_panic()),
            }
        } else {
            Self::Cancelled
        }
    }
}

impl fmt::Display for AsyncTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Panicked { message } => write!(formatter, "async task panicked: {message}"),
            Self::Cancelled => formatter.write_str("async task was cancelled"),
        }
    }
}

impl std::error::Error for AsyncTaskError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum AsyncRuntimeError {
    RuntimePluginMissing,
    AsyncRuntimePluginMissing,
    AsyncRuntimePluginAlreadyInstalled,
    RequestAlreadyRegistered { request_type: &'static str },
    ExecutorThreadStartFailed { source: io::Error },
    ExecutorRuntimeBuildFailed { source: io::Error },
    ExecutorDisconnected,
}

impl AsyncRuntimeError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for AsyncRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimePluginMissing => formatter.write_str("RuntimePlugin is not installed"),
            Self::AsyncRuntimePluginMissing => {
                formatter.write_str("AsyncRuntimePlugin is not installed")
            }
            Self::AsyncRuntimePluginAlreadyInstalled => {
                formatter.write_str("AsyncRuntimePlugin is already installed")
            }
            Self::RequestAlreadyRegistered { request_type } => {
                write!(
                    formatter,
                    "async request `{request_type}` is already registered"
                )
            }
            Self::ExecutorThreadStartFailed { source } => {
                write!(formatter, "failed to start async executor thread: {source}")
            }
            Self::ExecutorRuntimeBuildFailed { source } => {
                write!(
                    formatter,
                    "failed to build async executor runtime: {source}"
                )
            }
            Self::ExecutorDisconnected => formatter.write_str("async executor is disconnected"),
        }
    }
}

impl std::error::Error for AsyncRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExecutorThreadStartFailed { source }
            | Self::ExecutorRuntimeBuildFailed { source } => Some(source),
            _ => None,
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send + 'static>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).into()
    } else {
        "non-string panic payload".into()
    }
}
