use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use axum::http::Method;
use axum::Router;
use core_plugin::Resource;
use tokio::sync::oneshot;

use crate::{ServerError, WebSocketMessageClassifier};

pub(crate) type ErasedWebSocketClassifier =
    Arc<dyn WebSocketMessageClassifier + Send + Sync + 'static>;

pub(crate) struct EventRoute {
    pub(crate) method: Method,
    pub(crate) path: String,
}

pub(crate) struct WebSocketRoute {
    pub(crate) path: String,
    pub(crate) classifier: ErasedWebSocketClassifier,
}

struct RouteRegistryState {
    native_router: Option<Router>,
    event_routes: Vec<EventRoute>,
    websocket_routes: Vec<WebSocketRoute>,
    frozen: bool,
}

pub(crate) struct RouteRegistry {
    state: Mutex<RouteRegistryState>,
}

impl RouteRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(RouteRegistryState {
                native_router: Some(Router::new()),
                event_routes: Vec::new(),
                websocket_routes: Vec::new(),
                frozen: false,
            }),
        }
    }

    pub(crate) fn merge(&self, router: Router) {
        let mut state = self
            .state
            .lock()
            .expect("server route registry lock poisoned");
        if state.frozen {
            ServerError::RoutesFrozen.panic();
        }
        let current = state.native_router.take().unwrap_or_default();
        state.native_router = Some(current.merge(router));
    }

    pub(crate) fn add_event_route(&self, method: Method, path: String) {
        let mut state = self
            .state
            .lock()
            .expect("server route registry lock poisoned");
        if state.frozen {
            ServerError::RoutesFrozen.panic();
        }
        if state
            .event_routes
            .iter()
            .any(|route| route.method == method && route.path == path)
        {
            ServerError::EventRouteAlreadyRegistered { method, path }.panic();
        }
        state.event_routes.push(EventRoute { method, path });
    }

    pub(crate) fn add_websocket_route(&self, path: String, classifier: ErasedWebSocketClassifier) {
        let mut state = self
            .state
            .lock()
            .expect("server route registry lock poisoned");
        if state.frozen {
            ServerError::RoutesFrozen.panic();
        }
        if state
            .websocket_routes
            .iter()
            .any(|route| route.path == path)
        {
            ServerError::WebSocketRouteAlreadyRegistered { path }.panic();
        }
        state
            .websocket_routes
            .push(WebSocketRoute { path, classifier });
    }

    pub(crate) fn freeze(&self) -> (Router, Vec<EventRoute>, Vec<WebSocketRoute>) {
        let mut state = self
            .state
            .lock()
            .expect("server route registry lock poisoned");
        if state.frozen {
            ServerError::RoutesFrozen.panic();
        }
        state.frozen = true;
        (
            state.native_router.take().unwrap_or_default(),
            std::mem::take(&mut state.event_routes),
            std::mem::take(&mut state.websocket_routes),
        )
    }
}

impl Resource for RouteRegistry {}

struct ServerHandleState {
    local_address: RwLock<Option<SocketAddr>>,
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>,
}

impl Drop for ServerHandleState {
    fn drop(&mut self) {
        if let Some(sender) = self
            .shutdown_sender
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(());
        }
    }
}

#[derive(Clone)]
pub struct ServerHandle {
    state: Arc<ServerHandleState>,
}

impl ServerHandle {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(ServerHandleState {
                local_address: RwLock::new(None),
                shutdown_sender: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn set_local_address(&self, address: SocketAddr) {
        *self
            .state
            .local_address
            .write()
            .expect("server address lock poisoned") = Some(address);
    }

    pub(crate) fn set_shutdown_sender(&self, sender: oneshot::Sender<()>) {
        *self
            .state
            .shutdown_sender
            .lock()
            .expect("server shutdown lock poisoned") = Some(sender);
    }

    pub(crate) fn mark_stopped(&self) {
        *self
            .state
            .local_address
            .write()
            .expect("server address lock poisoned") = None;
        self.state
            .shutdown_sender
            .lock()
            .expect("server shutdown lock poisoned")
            .take();
    }

    pub fn local_address(&self) -> Option<SocketAddr> {
        *self
            .state
            .local_address
            .read()
            .expect("server address lock poisoned")
    }

    pub fn is_running(&self) -> bool {
        self.local_address().is_some()
    }

    pub fn shutdown(&self) {
        if let Some(sender) = self
            .state
            .shutdown_sender
            .lock()
            .expect("server shutdown lock poisoned")
            .take()
        {
            let _ = sender.send(());
        }
    }
}

impl Default for ServerHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl Resource for ServerHandle {}
