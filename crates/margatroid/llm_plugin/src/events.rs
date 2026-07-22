use types::{ChatRequest, ChatResponse, StreamChunk};

#[derive(Clone)]
pub struct LlmRequest {
    pub request_id: String,
    pub provider: String,
    pub request: ChatRequest,
}

impl LlmRequest {
    pub fn new(
        request_id: impl Into<String>,
        provider: impl Into<String>,
        request: ChatRequest,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            provider: provider.into(),
            request,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub request_id: String,
    pub response: ChatResponse,
}

#[derive(Clone, Debug)]
pub struct LlmStreamChunk {
    pub request_id: String,
    pub chunk: StreamChunk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmFailed {
    pub request_id: String,
    pub provider: String,
    pub kind: LlmFailureKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LlmFailureKind {
    ProviderNotFound,
    RequestFailed,
    StreamFailed,
}
