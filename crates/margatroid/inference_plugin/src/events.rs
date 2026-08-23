use async_runtime_plugin::AsyncTaskError;
use core_plugin::{Entity, Event};
use margatroid_types::{Message, ResourceId};
use server_plugin::WebSocketSender;
use tokio::sync::watch;

use crate::error::InferenceError;
use crate::types::{
    ErasedProviderAdapter, ModelRoutesReloaded, ProviderHttpRequest, ProviderInferenceResponse,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadModelRoutes {
    pub id: String,
}

impl Event for ReloadModelRoutes {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadModelRoutesResult {
    pub id: String,
    pub result: Result<ModelRoutesReloaded, InferenceError>,
}

impl Event for ReloadModelRoutesResult {}

pub use margatroid_types::InferenceRequestEvent;

#[derive(Clone, Debug)]
pub struct ContextCompactionInferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub agent_id: ResourceId,
    pub messages: Vec<Message>,
}

impl Event for ContextCompactionInferenceRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCompactionInferenceResponse {
    pub id: String,
    pub agent: Entity,
    pub result: Result<String, InferenceError>,
}

impl Event for ContextCompactionInferenceResponse {}

pub use margatroid_types::{CapturedInferenceRequest, CapturedInferenceResponse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelInferenceRequest {
    pub id: String,
    pub agent: Entity,
}

impl Event for CancelInferenceRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InferenceRoute {
    pub(crate) id: String,
    pub(crate) agent: Entity,
    pub(crate) output: InferenceOutputKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InferenceOutputKind {
    AgentMessage,
    ContextCompaction,
    Captured,
}

pub(crate) struct PreparedInference {
    pub(crate) route: InferenceRoute,
    pub(crate) agent_id: ResourceId,
    pub(crate) client: reqwest::Client,
    pub(crate) request: ProviderHttpRequest,
    pub(crate) adapter: ErasedProviderAdapter,
    pub(crate) senders: Vec<WebSocketSender>,
    pub(crate) cancellation: watch::Receiver<bool>,
}

impl Event for PreparedInference {}

#[derive(Clone)]
pub(crate) struct InferenceTaskOutput {
    pub(crate) route: InferenceRoute,
    pub(crate) result: InferenceTaskResult,
}

#[derive(Clone)]
pub(crate) enum InferenceTaskResult {
    Completed(Result<ProviderInferenceResponse, InferenceError>),
    Cancelled,
}

pub(crate) struct InferenceTaskError {
    pub(crate) source: AsyncTaskError,
}

impl From<AsyncTaskError> for InferenceTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}
