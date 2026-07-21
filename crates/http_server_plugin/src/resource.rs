use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use axum::Router;
use tokio::sync::watch;

use crate::HttpServerOptions;

pub(crate) struct HttpRoutes {
    router: Mutex<Router>,
}

impl HttpRoutes {
    pub(crate) fn new() -> Self {
        Self {
            router: Mutex::new(Router::new()),
        }
    }

    pub(crate) fn merge(&self, router: Router) {
        let mut routes = self
            .router
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *routes = routes.clone().merge(router);
    }

    pub(crate) fn snapshot(&self) -> Router {
        self.router
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HttpServerState {
    #[default]
    Stopped,
    Running(SocketAddr),
}

struct HttpServerInner {
    state: Mutex<HttpServerState>,
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct HttpServerHandle {
    inner: Arc<HttpServerInner>,
}

impl HttpServerHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(HttpServerInner {
                state: Mutex::new(HttpServerState::Stopped),
                shutdown: Mutex::new(None),
                thread: Mutex::new(None),
            }),
        }
    }

    pub fn state(&self) -> HttpServerState {
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn address(&self) -> Option<SocketAddr> {
        match self.state() {
            HttpServerState::Stopped => None,
            HttpServerState::Running(address) => Some(address),
        }
    }

    pub fn shutdown(&self) {
        if let Some(sender) = self
            .inner
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(true);
        }
        if let Some(thread) = self
            .inner
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = HttpServerState::Stopped;
    }

    pub(crate) fn start(
        &self,
        options: HttpServerOptions,
        router: Router,
    ) -> Result<SocketAddr, HttpServerError> {
        if let Some(address) = self.address() {
            return Ok(address);
        }

        let (startup_sender, startup_receiver) = std::sync::mpsc::sync_channel(1);
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let shutdown_timeout = options.shutdown_timeout;
        let thread = std::thread::Builder::new()
            .name("mecs-http-server".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = startup_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(options.bind_address).await {
                        Ok(listener) => listener,
                        Err(error) => {
                            let _ = startup_sender.send(Err(error.to_string()));
                            return;
                        }
                    };
                    let address = match listener.local_addr() {
                        Ok(address) => address,
                        Err(error) => {
                            let _ = startup_sender.send(Err(error.to_string()));
                            return;
                        }
                    };
                    if startup_sender.send(Ok(address)).is_err() {
                        return;
                    }
                    let graceful_receiver = shutdown_receiver.clone();
                    let shutdown = wait_for_shutdown(graceful_receiver);
                    let deadline = async move {
                        wait_for_shutdown(shutdown_receiver).await;
                        tokio::time::sleep(shutdown_timeout).await;
                    };
                    let server = axum::serve(listener, router).with_graceful_shutdown(shutdown);
                    tokio::select! {
                        result = server => {
                            if let Err(error) = result {
                                tracing::error!(%error, "HTTP server stopped with an error");
                            }
                        }
                        _ = deadline => {
                            tracing::warn!("HTTP server exceeded graceful shutdown timeout");
                        }
                    }
                });
            })
            .map_err(|error| HttpServerError(error.to_string()))?;

        let address = match startup_receiver.recv() {
            Ok(Ok(address)) => address,
            Ok(Err(message)) => {
                let _ = thread.join();
                return Err(HttpServerError(message));
            }
            Err(error) => {
                let _ = thread.join();
                return Err(HttpServerError(error.to_string()));
            }
        };
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = HttpServerState::Running(address);
        *self
            .inner
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shutdown_sender);
        *self
            .inner
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(thread);
        Ok(address)
    }
}

impl Default for HttpServerHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HttpServerInner {
    fn drop(&mut self) {
        if let Some(sender) = self
            .shutdown
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(true);
        }
        if let Some(thread) = self
            .thread
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

#[derive(Debug)]
pub(crate) struct HttpServerError(String);

impl fmt::Display for HttpServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
