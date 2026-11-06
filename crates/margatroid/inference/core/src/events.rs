use crate::{InferenceError, InferenceMessage, InferenceParameters, InferenceResponse};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
pub type InferenceReply<T> = Arc<Mutex<Option<oneshot::Sender<Result<T, InferenceError>>>>>;
#[derive(Clone)]
pub struct InitInference {
    pub agent: Entity,
    pub reply: InferenceReply<()>,
}
impl Event for InitInference {}
#[derive(Clone)]
pub struct InferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub turn: String,
    pub messages: Vec<InferenceMessage>,
    pub config: InferenceParameters,
    pub cancel: CancellationToken,
    pub reply: InferenceReply<InferenceResponse>,
}
impl Event for InferenceRequest {}
