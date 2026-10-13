use crate::block_types::{BlockInner, InnerType};
use crate::execution_components::AgentMclDrainHandle;
use crate::history_components::HistoryError;
use crate::message_types::InferenceResponse;
use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclInstruction {
    pub text: String,
    #[serde(default)]
    pub arguments: Vec<serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclCommand {
    pub id: String,
    pub source: ResourceID,
    pub target: ResourceID,
    pub command: MclInstruction,
}
impl Event for MclCommand {}
#[derive(Clone, Debug)]
pub struct InternalMclHostSubmission {
    pub id: String,
    pub instance: crate::lua_runtime_components::DriverLuaVmInstance,
    pub cancel: CancellationToken,
    pub target: ResourceID,
    pub command: MclInstruction,
    pub reply:
        std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<MclCommandResult>>>>,
}
impl Event for InternalMclHostSubmission {}

#[derive(Clone, Debug)]
pub struct InternalMclCancellationSignal;
impl Event for InternalMclCancellationSignal {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclCommandResult {
    pub id: String,
    pub source: ResourceID,
    pub target: ResourceID,
    pub result: Result<MclValue, MclError>,
    pub history_error: Option<HistoryError>,
}
impl Event for MclCommandResult {}
impl MclCommandResult {
    pub fn from_request(request: &MclCommand, result: Result<MclValue, MclError>) -> Self {
        Self {
            id: request.id.clone(),
            source: request.source.clone(),
            target: request.target.clone(),
            result,
            history_error: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MclValue {
    Unit,
    Array(BlockInner),
    ResourceImport {
        kind: InnerType,
        resource_id: ResourceID,
        alias: String,
    },
    Inference(InferenceResponse),
    Text(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MclError {
    InvalidCommand,
    InvalidRequest,
    DuplicateRequest,
    AgentMissing,
    AgentStopping,
    TypeMismatch,
    InvalidRange,
    BindingMissing,
    InvalidResourceId,
    SourceReadFailed,
    EffectInvalid,
    ToolCallInvalid,
    HistoryWriteFailed(String),
    ImportNotAllowed,
    ImportKindMismatch,
    AliasMissing,
    ResourceMissing,
    AliasConflict,
    Cancelled,
    ExecutionFailed(String),
}
impl std::fmt::Display for MclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MclError {}
#[derive(Clone, Debug)]
pub struct MclPending {
    pub command_id: String,
    pub target_entity: Entity,
    pub operation_id: String,
    pub cancel: CancellationToken,
}
#[derive(Debug)]
pub struct AgentMclQueue {
    pub commands: VecDeque<MclCommand>,
    pub pending: Option<MclPending>,
    pub drained: AgentMclDrainHandle,
    pub accepting: bool,
}
impl Default for AgentMclQueue {
    fn default() -> Self {
        Self {
            commands: VecDeque::new(),
            pending: None,
            drained: AgentMclDrainHandle::default(),
            accepting: true,
        }
    }
}
#[derive(Clone, Debug)]
pub struct MclAsyncOutput {
    pub request: MclCommand,
    pub pending: MclPending,
    pub turn: Option<String>,
    pub result: Result<MclValue, MclError>,
}
impl Event for MclAsyncOutput {}
