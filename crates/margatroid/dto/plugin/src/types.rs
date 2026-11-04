use client_core::{ClientFrame, ClientSender, ConnectionId};
use config_core::OutboundTarget;
use core_plugin::{Entity, Event, Resource};
use margatroid_protocol::{
    AgentHistoryDto, BackendStateDto, LogRecordDto, ProtocolErrorDto, WorkspaceMemberFailureDto,
    WorkspaceReferenceDto,
};
use mcl_core::MclValue;
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTarget {
    pub workspace: Option<Entity>,
    pub entity: Entity,
    pub instance: Option<String>,
    pub address: String,
}

#[derive(Clone, Debug)]
pub(crate) enum OperationKind {
    Registration,
    WorkspaceStart,
    WorkspaceRestart,
    WorkspaceStop {
        reference: WorkspaceReferenceDto,
    },
    AgentStart {
        workspace: Entity,
        name: String,
    },
    AgentRestart {
        workspace: Entity,
        name: String,
    },
    Mcl {
        source: String,
        target: String,
    },
    History,
    State,
    Abort {
        workspace: WorkspaceReferenceDto,
        agent: String,
        turn: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RequestContext {
    pub network_id: String,
    pub operation_id: String,
    pub client: Option<Entity>,
    pub connection: ConnectionId,
    pub target: Option<LiveTarget>,
    pub kind: OperationKind,
}

#[derive(Default)]
pub(crate) struct PendingRequests {
    pub entries: BTreeMap<String, RequestContext>,
}
impl Resource for PendingRequests {}

#[derive(Clone, Debug)]
pub(crate) enum AsyncPayload {
    Mcl(Result<MclValue, ProtocolErrorDto>),
    Agent(Result<Entity, ProtocolErrorDto>),
    History(Result<AgentHistoryDto, ProtocolErrorDto>),
}
#[derive(Clone, Debug)]
pub(crate) struct ResponseReady {
    pub operation_id: String,
    pub payload: AsyncPayload,
}
impl Event for ResponseReady {}

#[derive(Clone, Debug)]
pub(crate) struct AbortTurnControl {
    pub operation_id: String,
    pub agent: Entity,
    pub workspace: Entity,
    pub instance: String,
    pub turn: String,
}
impl Event for AbortTurnControl {}

#[derive(Clone, Debug)]
pub(crate) struct SnapshotReady {
    pub token: u64,
    pub result: Result<Vec<(LiveTarget, AgentHistoryDto)>, ProtocolErrorDto>,
}
impl Event for SnapshotReady {}

#[derive(Default)]
pub(crate) struct BackendStateReportCache {
    pub state: Option<BackendStateDto>,
    pub metadata: Option<BackendStateDto>,
    pub version: u64,
    pub failed_state_version: Option<u64>,
    pub recipients: BTreeMap<u64, (Entity, u64)>,
    pub registered: BTreeMap<u64, Entity>,
    pub pending: Option<u64>,
    pub dirty: bool,
    pub last_refresh: Option<Instant>,
    pub retry_after: Option<Instant>,
    pub state_waiters: Vec<String>,
    pub failures: BTreeMap<Entity, Vec<WorkspaceMemberFailureDto>>,
    pub errors: BTreeMap<Entity, ProtocolErrorDto>,
}
impl Resource for BackendStateReportCache {}

#[derive(Clone, Debug)]
pub struct WebSocketMessageSend {
    pub target: OutboundTarget,
    pub message: serde_json::Value,
}
impl Event for WebSocketMessageSend {}

#[derive(Clone, Debug)]
pub(crate) struct LogReady(pub LogRecordDto);
impl Event for LogReady {}

#[derive(Clone, Debug)]
pub(crate) struct DtoRequest {
    pub connection: ConnectionId,
    pub message: margatroid_protocol::ClientMessage,
}
impl Event for DtoRequest {}
#[derive(Clone, Debug)]
pub(crate) struct DtoAsyncError(pub String);
impl From<async_runtime_plugin::AsyncTaskError> for DtoAsyncError {
    fn from(error: async_runtime_plugin::AsyncTaskError) -> Self {
        Self(error.to_string())
    }
}
#[derive(Clone)]
pub(crate) struct DtoState {
    pub ingress: config_core::IngressPolicy,
    pub events: std::sync::Arc<std::sync::OnceLock<async_runtime_plugin::AsyncContext>>,
    pub connections: ConnState,
    pub lifetime: axum_plugin::AxumLifetime,
}
impl Resource for DtoState {}

#[derive(Clone, Debug)]
pub(crate) struct DtoInitialized;

#[derive(Clone, Default)]
pub(crate) struct ConnState {
    entries: std::sync::Arc<std::sync::Mutex<BTreeMap<ConnectionId, ConnectionEntry>>>,
}
struct ConnectionEntry {
    sender: ClientSender,
    cancel: tokio_util::sync::CancellationToken,
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
    pub fn cancel(&self, id: ConnectionId) {
        if let Some(entry) = self.entries.lock().unwrap().get(&id) {
            entry.cancel.cancel();
        }
    }
    pub fn remove(&self, id: ConnectionId) -> bool {
        self.entries.lock().unwrap().remove(&id).is_some()
    }
}

#[derive(Default)]
pub(crate) struct TransportStatus {
    pub address: Option<std::net::SocketAddr>,
    pub stopped: bool,
}
impl Resource for TransportStatus {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unregistered_connections_have_bounded_senders_and_independent_cancellation() {
        let state = ConnState::default();
        let (id, mut receiver, cancel) = state.allocate().unwrap();
        let (other, _other_receiver, other_cancel) = state.allocate().unwrap();
        assert_ne!(id, other);
        let sender = state.get(id).unwrap();
        for _ in 0..128 {
            sender.try_send(ClientFrame::Text("frame".into())).unwrap();
        }
        assert_eq!(
            sender.try_send(ClientFrame::Text(String::new())),
            Err(client_core::ClientSendError::Full)
        );
        state.cancel(id);
        assert!(cancel.is_cancelled());
        assert!(!other_cancel.is_cancelled());
        receiver.close();
        assert!(sender.is_closed());
        assert!(state.remove(id));
        assert!(!state.remove(id));
        assert!(state.get(id).is_none());
        assert!(state.get(other).is_some());
    }
    #[test]
    fn dropping_receiver_invalidates_sender_snapshots_before_identity_registration() {
        let state = ConnState::default();
        let (id, receiver, _cancel) = state.allocate().unwrap();
        let sender = state.get(id).unwrap();
        drop(receiver);
        assert!(sender.is_closed());
        assert_eq!(
            sender.try_send(ClientFrame::Binary(vec![1])),
            Err(client_core::ClientSendError::Closed)
        );
        assert!(state.remove(id));
    }
}
