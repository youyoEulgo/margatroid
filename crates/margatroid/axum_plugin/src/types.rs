use std::net::SocketAddr;

use async_runtime_plugin::{AsyncContext, AsyncTaskError};
use axum::Router;
use core_plugin::{Event, Resource};

use crate::{AxumError, AxumErrorKind, AxumHandle};

pub(crate) struct AxumBind(pub SocketAddr);
impl Resource for AxumBind {}

pub(crate) struct AxumContext {
    pub router: Router,
    pub bind: SocketAddr,
    pub handle: AxumHandle,
    pub events: Option<AsyncContext>,
    pub owner: Option<crate::handler::CompletionOwner>,
}

pub(crate) struct AxumStarted(pub SocketAddr);
impl Event for AxumStarted {}

pub(crate) struct AxumStopped;

pub(crate) struct AxumTaskError(pub AxumError);
impl From<AsyncTaskError> for AxumTaskError {
    fn from(error: AsyncTaskError) -> Self {
        let kind = match error {
            AsyncTaskError::Panicked { .. } => AxumErrorKind::Panicked,
            _ => AxumErrorKind::Cancelled,
        };
        Self(AxumError::new(kind, error.to_string()))
    }
}
