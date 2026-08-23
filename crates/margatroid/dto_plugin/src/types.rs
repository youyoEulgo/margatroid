use core_plugin::Resource;
use margatroid_protocol::BackendStateDto;
use server_plugin::WebSocketConnectionId;

#[derive(Default)]
pub(crate) struct BackendStateReportCache {
    pub(crate) state: Option<BackendStateDto>,
    pub(crate) recipients: Vec<u64>,
    pub(crate) last_error: Option<String>,
}

impl Resource for BackendStateReportCache {}

#[derive(Default)]
pub(crate) struct PendingMclCommandResponses {
    pub(crate) commands: Vec<PendingMclCommandResponse>,
}

impl Resource for PendingMclCommandResponses {}

pub(crate) struct PendingMclCommandResponse {
    pub(crate) id: String,
    pub(crate) connection_id: WebSocketConnectionId,
    pub(crate) response:
        std::sync::Mutex<std::sync::mpsc::Receiver<Result<serde_json::Value, String>>>,
}
