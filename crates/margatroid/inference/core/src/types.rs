use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct InferenceHandle {
    pub request_id: String,
    pub turn: String,
    pub cancel: CancellationToken,
    pub stop_stream: Arc<dyn Fn() + Send + Sync>,
    pub task: JoinHandle<()>,
}
impl InferenceHandle {
    pub fn cancel(&self) {
        (self.stop_stream)();
        self.cancel.cancel();
    }
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceParameters {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub model: String,
    pub provider: String,
    pub api_type: String,
    pub base_url: String,
    pub api_key: String,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}
impl std::fmt::Debug for ModelRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRoute")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("api_type", &self.api_type)
            .field("context_window_tokens", &self.context_window_tokens)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainRequest {
    pub messages: Vec<InferenceMessage>,
    pub tools: Vec<InferenceToolDefinition>,
    pub config: InferenceParameters,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InferenceMessage {
    System {
        content: String,
    },
    User {
        content: String,
        #[serde(default)]
        images: Vec<InferenceImage>,
    },
    Assistant {
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<InferenceToolCall>,
        replay: Option<Value>,
    },
    Tool {
        call_id: String,
        content: String,
        #[serde(default)]
        images: Vec<InferenceImage>,
        #[serde(default)]
        failed: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceImage {
    pub media_type: String,
    pub data: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Vec<InferenceToolCall>,
    pub replay: Option<Value>,
    pub usage: Option<InferenceUsage>,
    pub stop_reason: InferenceStopReason,
}
impl InferenceResponse {
    pub fn cancelled() -> Self {
        Self {
            reasoning: None,
            content: None,
            tool_calls: vec![],
            replay: None,
            usage: None,
            stop_reason: InferenceStopReason::Cancelled,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceStopReason {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Cancelled,
    Other(String),
}
impl InferenceStopReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::Cancelled => "cancelled",
            Self::Other(v) => v,
        }
    }
}
impl std::fmt::Display for InferenceStopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
