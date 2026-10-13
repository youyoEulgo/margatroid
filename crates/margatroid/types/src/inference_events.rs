use crate::message_types::{AgentMessage, InferenceResponse};
use crate::resource_components::CallableResource;
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceErrorKind {
    InvalidRequest,
    AgentMissing,
    Cancelled,
    PreparationFailed,
    ExecutionFailed,
    ResponseLost,
    InvalidModelId,
    ConfigReadFailed,
    ConfigDecodeFailed,
    DuplicateModelId,
    InvalidModelRoute,
    UnsupportedApiType,
    AgentNotAlive,
    ModelRouteNotFound,
    InvalidParameters,
    InvalidMessages,
    InvalidToolDefinitions,
    ResourceMissing,
    UnsupportedResource,
    UnsupportedInput,
    ConfigBusy,
    SubmitFailed,
    RequestBuildFailed,
    RequestFailed,
    ResponseStatus,
    ResponseDecodeFailed,
    ResponseEncodeFailed,
    ResponseIncomplete,
    ResponseTooLarge,
    Timeout,
    TaskPanicked,
    TaskAborted,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceError {
    pub kind: InferenceErrorKind,
    pub message: String,
}
impl InferenceError {
    pub fn new(kind: InferenceErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 512 {
            let mut boundary = 509;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str("...");
        }
        Self { kind, message }
    }
    pub fn kind(&self) -> InferenceErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn with_status(
        kind: InferenceErrorKind,
        status: Option<u16>,
        message: impl Into<String>,
    ) -> Self {
        let message = message.into();
        Self::new(
            kind,
            match status {
                Some(status) => format!("HTTP {status}: {message}"),
                None => message,
            },
        )
    }
}
impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for InferenceError {}
pub type InferenceReply =
    Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>;
#[derive(Clone, Debug)]
pub struct InferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<CallableResource>,
    pub turn: Option<String>,
    pub cancel: CancellationToken,
    pub reply: InferenceReply,
}
impl Event for InferenceRequest {}

#[derive(Clone, Debug)]
pub struct ReloadModelRoutes {
    pub id: String,
}
impl Event for ReloadModelRoutes {}
#[derive(Clone, Debug)]
pub struct ReloadModelRoutesResult {
    pub id: String,
    pub result: Result<usize, InferenceError>,
}
impl Event for ReloadModelRoutesResult {}
#[derive(Clone, Debug)]
pub struct InferenceReloadOutput {
    pub id: String,
    pub result: Result<String, InferenceError>,
}
impl Event for InferenceReloadOutput {}
