use core_plugin::{Entity, Resource};
use margatroid_protocol::{BackendStateDto, WorkspaceReferenceDto};
use resource_id_plugin::ResourceID;
use server_plugin::WebSocketConnectionId;

#[derive(Default)]
pub(crate) struct BackendStateReportCache {
    pub(crate) state: Option<BackendStateDto>,
    pub(crate) recipients: Vec<u64>,
    pub(crate) last_error: Option<String>,
    pub(crate) pending: Option<String>,
    pub(crate) prepared_state: Option<BackendStateDto>,
    pub(crate) dirty: bool,
}
impl Resource for BackendStateReportCache {}
#[derive(Default)]
pub(crate) struct PendingMclCommandResponses {
    pub(crate) aborts: Vec<PendingAbortResponse>,
    pub(crate) commands: Vec<PendingMclCommandResponse>,
    pub(crate) workspaces: Vec<PendingWorkspaceResponse>,
    pub(crate) snapshots: Vec<PendingSnapshot>,
    pub(crate) catalog: Vec<PendingCatalogResponse>,
    pub(crate) discovery: Vec<PendingDiscoveryResponse>,
}
pub(crate) struct PendingSnapshot {
    pub(crate) operation_id: String,
    pub(crate) id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) history: bool,
}
impl Resource for PendingMclCommandResponses {}
pub(crate) struct PendingMclCommandResponse {
    pub(crate) id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) source: ResourceID,
    pub(crate) target: ResourceID,
}
pub(crate) struct PendingWorkspaceResponse {
    pub(crate) id: String,
    pub(crate) operation_id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) stopping: bool,
    pub(crate) reference: Option<WorkspaceReferenceDto>,
}

pub(crate) struct PendingAbortResponse {
    pub(crate) id: String,
    pub(crate) operation_id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) agent: Entity,
    pub(crate) resource_id: ResourceID,
    pub(crate) workspace: WorkspaceReferenceDto,
    pub(crate) instance: margatroid_types::lua_runtime_components::DriverLuaVmInstance,
    pub(crate) turn_id: String,
}

pub(crate) struct PendingDiscoveryResponse {
    pub(crate) operation_id: String,
    pub(crate) id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) kind: DiscoveryOperation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscoveryOperation {
    Paths,
    DriverQuery,
    DriverRefresh,
}

pub(crate) struct PendingCatalogResponse {
    pub(crate) operation_id: String,
    pub(crate) id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) client: Entity,
    pub(crate) kind: CatalogOperation,
}
/// Catalog-definition operation that replies with `workspace.member.result`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogOperation {
    Query,
    Refresh,
    MemberAdd,
    MemberRemove,
    Create,
    Rename,
    Delete,
    MemberRename,
}
impl CatalogOperation {
    /// Whole-catalog replies use `workspace.catalog`; definition edits use the entry reply.
    pub(crate) fn entry_reply(self) -> bool {
        !matches!(self, Self::Query | Self::Refresh)
    }
}
