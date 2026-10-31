use config_core::OutboundTarget;
use core_plugin::{Entity, Event, Resource};
use margatroid_protocol::{
    AgentHistoryDto, BackendStateDto, LogRecordDto, ProtocolErrorDto, WorkspaceMemberFailureDto,
    WorkspaceReferenceDto,
};
use mcl_core::MclValue;
use server_plugin::WebSocketConnectionId;
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
    pub connection: WebSocketConnectionId,
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
pub(crate) enum TransportEvent {
    Started(std::net::SocketAddr),
    Failed,
    Stopped,
    Connected(WebSocketConnectionId),
    ProtocolFailed(WebSocketConnectionId),
}

#[derive(Clone, Debug)]
pub(crate) struct LogReady(pub LogRecordDto);
impl Event for LogReady {}
