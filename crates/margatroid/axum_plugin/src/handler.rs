use std::panic::AssertUnwindSafe;

use axum::Extension;
use futures_util::FutureExt;
use tokio::net::TcpListener;

use crate::types::{AxumContext, AxumStarted, AxumStopped, AxumTaskError};
use crate::{AxumError, AxumErrorKind, AxumHandle};

pub(crate) async fn run_axum(mut context: AxumContext) -> Result<AxumStopped, AxumTaskError> {
    let owner = context.owner.take().unwrap();
    let result = AssertUnwindSafe(serve(context)).catch_unwind().await;
    let result = match result {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|value| (*value).to_owned())
                })
                .unwrap_or_else(|| "non-string panic payload".into());
            Err(AxumError::new(AxumErrorKind::Panicked, message))
        }
    };
    owner.0.request_shutdown();
    owner.0.lifetime.connections.close();
    owner.0.lifetime.connections.wait().await;
    owner.0.complete(result.clone());
    result.map(|()| AxumStopped).map_err(AxumTaskError)
}

async fn serve(context: AxumContext) -> Result<(), AxumError> {
    let listener = TcpListener::bind(context.bind)
        .await
        .map_err(|error| AxumError::new(AxumErrorKind::Bind, error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| AxumError::new(AxumErrorKind::Bind, error.to_string()))?;
    *context.handle.address.write().unwrap() = Some(address);
    context
        .events
        .as_ref()
        .unwrap()
        .send_event(AxumStarted(address));
    let lifetime = context.handle.lifetime();
    let shutdown = lifetime.shutdown.clone();
    axum::serve(
        listener,
        context
            .router
            .layer(Extension(lifetime))
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
    .map_err(|error| AxumError::new(AxumErrorKind::Serve, error.to_string()))
}

pub(crate) struct CompletionOwner(pub(crate) AxumHandle);
impl Drop for CompletionOwner {
    fn drop(&mut self) {
        self.0.request_shutdown();
        if !self.0.is_stopped() {
            self.0.complete(Err(AxumError::new(
                AxumErrorKind::Cancelled,
                "Axum service task dropped before cleanup completed",
            )));
        }
    }
}
