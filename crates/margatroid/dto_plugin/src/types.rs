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
    pub(crate) commands: Vec<PendingMclCommandResponse>,
    pub(crate) workspaces: Vec<PendingWorkspaceResponse>,
    pub(crate) snapshots: Vec<PendingSnapshot>,
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
    pub(crate) reference: WorkspaceReferenceDto,
}
