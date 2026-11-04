mod error;
mod handler;
mod system;
mod types;

pub use error::{AxumError, AxumErrorKind};

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use axum::Router;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};
use tokio::sync::watch;
use tokio_util::{
    sync::CancellationToken,
    task::{task_tracker::TaskTrackerToken, TaskTracker},
};

#[derive(Clone, Debug)]
pub struct AxumPlugin {
    bind: SocketAddr,
}

impl AxumPlugin {
    pub fn bind(bind: SocketAddr) -> Self {
        Self { bind }
    }
    pub fn new(bind: SocketAddr) -> Self {
        Self::bind(bind)
    }
}

impl Plugin for AxumPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
            || app.world().contains_resource::<AxumHandle>()
        {
            panic!("{}", AxumError::new(AxumErrorKind::Installation, "AxumPlugin requires RuntimePlugin, AsyncRuntimePlugin and a single installation"));
        }
        app.world().closure_sender(RuntimePlugin::UPDATE);
        let handle = AxumHandle::new();
        app.world_mut()
            .insert_resource(AxumOwner(handle.lifetime.shutdown.clone()));
        app.world_mut().insert_resource(handle);
        app.world_mut().insert_resource(AxumRoutes {
            router: Some(Router::new()),
        });
        app.world_mut().insert_resource(types::AxumBind(self.bind));
        app.add_system(RuntimePlugin::STARTUP, system::start_axum_system)
            .add_system(RuntimePlugin::UPDATE, system::axum_result_system);
    }
}

pub trait AppAxumExt {
    fn add_axum_routes(&mut self, router: Router) -> &mut Self;
}

impl AppAxumExt for App {
    fn add_axum_routes(&mut self, router: Router) -> &mut Self {
        let routes = self
            .world_mut()
            .get_resource_mut::<AxumRoutes>()
            .unwrap_or_else(|| {
                panic!(
                    "{}",
                    AxumError::new(AxumErrorKind::Installation, "AxumPlugin is not installed")
                )
            });
        let current = routes.router.take().unwrap_or_else(|| {
            panic!(
                "{}",
                AxumError::new(
                    AxumErrorKind::RoutesFrozen,
                    "Axum routes are frozen after startup"
                )
            )
        });
        routes.router = Some(current.merge(router));
        self
    }
}

pub struct AxumRoutes {
    pub(crate) router: Option<Router>,
}
impl Resource for AxumRoutes {}

#[derive(Clone)]
pub struct AxumLifetime {
    pub(crate) shutdown: CancellationToken,
    pub(crate) connections: TaskTracker,
}

impl AxumLifetime {
    pub async fn cancelled(&self) {
        self.shutdown.cancelled().await;
    }
    pub fn is_cancelled(&self) -> bool {
        self.shutdown.is_cancelled()
    }
    pub fn track(&self) -> AxumWorkGuard {
        AxumWorkGuard {
            _token: self.connections.token(),
        }
    }
}

pub struct AxumWorkGuard {
    _token: TaskTrackerToken,
}

#[derive(Clone)]
pub struct AxumHandle {
    pub(crate) lifetime: AxumLifetime,
    pub(crate) address: Arc<RwLock<Option<SocketAddr>>>,
    completion: watch::Sender<Option<Result<(), AxumError>>>,
}

impl AxumHandle {
    fn new() -> Self {
        let (completion, _) = watch::channel(None);
        Self {
            lifetime: AxumLifetime {
                shutdown: CancellationToken::new(),
                connections: TaskTracker::new(),
            },
            address: Arc::new(RwLock::new(None)),
            completion,
        }
    }
    pub fn local_address(&self) -> Option<SocketAddr> {
        *self.address.read().unwrap()
    }
    pub fn lifetime(&self) -> AxumLifetime {
        self.lifetime.clone()
    }
    pub fn request_shutdown(&self) {
        self.lifetime.shutdown.cancel();
    }
    pub fn is_stopped(&self) -> bool {
        self.completion.borrow().is_some()
    }
    pub fn result(&self) -> Option<Result<(), AxumError>> {
        self.completion.borrow().clone()
    }
    pub async fn wait_stopped(&self) -> Result<(), AxumError> {
        let mut completion = self.completion.subscribe();
        loop {
            if let Some(result) = completion.borrow_and_update().clone() {
                return result;
            }
            if completion.changed().await.is_err() {
                return Err(AxumError::new(
                    AxumErrorKind::Cancelled,
                    "Axum completion channel closed",
                ));
            }
        }
    }
    pub(crate) fn complete(&self, result: Result<(), AxumError>) {
        self.completion.send_if_modified(|current| {
            if current.is_some() {
                false
            } else {
                *current = Some(result);
                true
            }
        });
    }
}
impl Resource for AxumHandle {}

struct AxumOwner(CancellationToken);
impl Resource for AxumOwner {}
impl Drop for AxumOwner {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
