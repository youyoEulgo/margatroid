use core_plugin::{Entity, Event, Resource};
use margatroid_protocol::*;
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

#[derive(Clone)]
pub(crate) struct DtoState {
    pub ingress: config_core::IngressPolicy,
    pub events: Arc<std::sync::OnceLock<async_runtime_plugin::AsyncContext>>,
    pub clients: Clients,
    pub lifetime: axum_plugin::AxumLifetime,
    pub definitions: workspace_core::WorkspaceDefinitions,
}
impl Resource for DtoState {}

pub(crate) type Ticket = client_core::RequestTicket;
pub(crate) type PreparedReply<R, E> = Arc<Mutex<Option<oneshot::Sender<Result<R, E>>>>>;
pub(crate) struct Prepared<T, R, E = ProtocolErrorDto> {
    pub ticket: Arc<Ticket>,
    pub input: T,
    pub reply: PreparedReply<R, E>,
}
#[derive(Clone)]
pub(crate) struct DtoRequest {
    pub route: String,
    pub object: Arc<Mutex<Option<Box<dyn Any + Send>>>>,
}
impl Event for DtoRequest {}
#[derive(Clone)]
pub(crate) struct AbortTurnControl {
    pub request: Arc<Mutex<Option<Prepared<AgentTurnAbortDto, ()>>>>,
}
impl Event for AbortTurnControl {}
#[derive(Clone, Debug)]
pub(crate) struct DtoInitialized;
#[derive(Clone, Debug)]
pub(crate) struct DtoAsyncError(pub String);
impl From<async_runtime_plugin::AsyncTaskError> for DtoAsyncError {
    fn from(error: async_runtime_plugin::AsyncTaskError) -> Self {
        Self(error.to_string())
    }
}
#[derive(Default)]
pub(crate) struct SyncObservations {
    pub records: BTreeMap<Entity, RecordWatchSlot>,
    pub ui: BTreeMap<Entity, UiWatchSlot>,
}
impl Resource for SyncObservations {}
pub(crate) struct RecordWatchSlot {
    pub completion: Arc<crate::monitor::MonitorCompletion>,
    pub storage: storage_core::AgentStorage,
    pub cancel: tokio_util::sync::CancellationToken,
    pub active: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for RecordWatchSlot {
    fn drop(&mut self) {
        self.active
            .store(false, std::sync::atomic::Ordering::Release);
        self.cancel.cancel();
    }
}
pub(crate) struct RecordMonitorContext {
    pub completion: crate::monitor::MonitorCompletionGuard,
    pub storage: storage_core::AgentStorage,
    pub address: String,
    pub clients: client_core::Clients,
    pub targets: Arc<[client_core::ClientSelector]>,
    pub active: Arc<std::sync::atomic::AtomicBool>,
    pub changes: storage_core::RecordChanges,
    pub position: storage_core::RecordWatermarks,
    pub cancel: tokio_util::sync::CancellationToken,
    pub lifetime: axum_plugin::AxumLifetime,
    pub guard: axum_plugin::AxumWorkGuard,
}
pub(crate) use client_core::Clients;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UiDeliveryKey {
    Initializing,
    Missing,
    Ready(String),
    Failed { code: String, message: String },
}
#[derive(Clone)]
pub(crate) struct UiDelivery {
    pub key: UiDeliveryKey,
    pub frame: Arc<str>,
}
pub(crate) struct UiWatchSlot {
    pub completion: Arc<crate::monitor::MonitorCompletion>,
    pub manifest_path: std::path::PathBuf,
    pub address: String,
    pub watch: agent_driver_core::DriverUiWatch,
    pub sync: client_core::SyncContext<UiDelivery>,
}
impl Drop for UiWatchSlot {
    fn drop(&mut self) {
        self.sync.close();
        self.watch.cancel();
    }
}
pub(crate) struct UiMonitorContext {
    pub completion: crate::monitor::MonitorCompletionGuard,
    pub monitor: agent_driver_core::DriverUiMonitor,
    pub watch: agent_driver_core::DriverUiWatch,
    pub lifetime: axum_plugin::AxumLifetime,
    pub guard: axum_plugin::AxumWorkGuard,
}
