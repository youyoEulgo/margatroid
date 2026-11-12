use client_core::{ClientFrame, ClientSender, ConnectionId};
use config_core::OutboundTarget;
use core_plugin::{Entity, Event, Resource};
use margatroid_protocol::*;
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

#[derive(Clone)]
pub(crate) struct DtoState {
    pub ingress: config_core::IngressPolicy,
    pub events: Arc<std::sync::OnceLock<async_runtime_plugin::AsyncContext>>,
    pub connections: ConnState,
    pub lifetime: axum_plugin::AxumLifetime,
    pub definitions: workspace_core::WorkspaceDefinitions,
}
impl Resource for DtoState {}

pub(crate) struct Ticket {
    pub id: String,
    pub connection: ConnectionId,
    pub active: Arc<Mutex<BTreeSet<String>>>,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
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
pub struct WebSocketMessageSend {
    pub target: OutboundTarget,
    pub message: serde_json::Value,
}
impl Event for WebSocketMessageSend {}
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
pub(crate) struct SyncState {
    pub sent: BTreeMap<(u64, String), serde_json::Value>,
    pub errors: BTreeMap<Entity, ProtocolErrorDto>,
    pub records: BTreeMap<Entity, RecordWatchSlot>,
    pub ui: BTreeMap<Entity, UiWatchSlot>,
    pub ui_sent: BTreeMap<(ConnectionId, Entity), UiDeliveryKey>,
}
impl Resource for SyncState {}
pub(crate) struct RecordWatchSlot {
    pub storage: storage_core::AgentStorage,
    pub cancel: tokio_util::sync::CancellationToken,
}
impl Drop for RecordWatchSlot {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub(crate) struct RecordMonitorContext {
    pub storage: storage_core::AgentStorage,
    pub address: String,
    pub events: app_runtime_plugin::RuntimeEventSender,
    pub changes: storage_core::RecordChanges,
    pub position: storage_core::RecordWatermarks,
    pub cancel: tokio_util::sync::CancellationToken,
    pub lifetime: axum_plugin::AxumLifetime,
    pub guard: axum_plugin::AxumWorkGuard,
}
#[derive(Clone, Default)]
pub(crate) struct ConnState {
    entries: std::sync::Arc<std::sync::Mutex<BTreeMap<ConnectionId, ConnectionEntry>>>,
}
struct ConnectionEntry {
    sender: ClientSender,
    cancel: tokio_util::sync::CancellationToken,
    active: Arc<Mutex<BTreeSet<String>>>,
}
impl Resource for ConnState {}
impl ConnState {
    pub fn allocate(
        &self,
    ) -> Option<(
        ConnectionId,
        tokio::sync::mpsc::Receiver<ClientFrame>,
        tokio_util::sync::CancellationToken,
    )> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |id| id.checked_add(1),
            )
            .ok()?;
        let id = ConnectionId::new(id);
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let cancel = tokio_util::sync::CancellationToken::new();
        self.entries.lock().unwrap().insert(
            id,
            ConnectionEntry {
                sender: ClientSender::new(tx),
                cancel: cancel.clone(),
                active: Arc::new(Mutex::new(BTreeSet::new())),
            },
        );
        Some((id, rx, cancel))
    }
    pub fn get(&self, id: ConnectionId) -> Option<ClientSender> {
        self.entries
            .lock()
            .unwrap()
            .get(&id)
            .map(|entry| entry.sender.clone())
    }
    pub fn claim(&self, id: ConnectionId, request: &str) -> Option<Arc<Ticket>> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(&id)?;
        if !entry.active.lock().unwrap().insert(request.to_owned()) {
            return None;
        }
        Some(Arc::new(Ticket {
            id: request.to_owned(),
            connection: id,
            active: entry.active.clone(),
        }))
    }
    pub fn cancel(&self, id: ConnectionId) {
        if let Some(entry) = self.entries.lock().unwrap().get(&id) {
            entry.cancel.cancel();
        }
    }
    pub fn remove(&self, id: ConnectionId) -> bool {
        self.entries.lock().unwrap().remove(&id).is_some()
    }
}

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
    pub manifest_path: std::path::PathBuf,
    pub address: String,
    pub watch: agent_driver_core::DriverUiWatch,
    pub delivery: Arc<std::sync::RwLock<UiDelivery>>,
}
impl Drop for UiWatchSlot {
    fn drop(&mut self) {
        self.watch.cancel();
    }
}
pub(crate) struct UiMonitorContext {
    pub monitor: agent_driver_core::DriverUiMonitor,
    pub watch: agent_driver_core::DriverUiWatch,
    pub lifetime: axum_plugin::AxumLifetime,
    pub guard: axum_plugin::AxumWorkGuard,
}
