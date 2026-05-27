use crate::tool::{ResponseToolCall, ToolCallDelta};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    #[serde(other)]
    Other,
}

/// 非流式响应的单个 choice
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseChoice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: Option<FinishReason>,
}

/// 流式响应的单个 choice delta
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: ResponseDelta,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,

    // 扩展字段，不是所有 provider 都会返回
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,

    // OpenRouter only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PromptTokensDetails {
    pub cached_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompletionTokensDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
}

/// 非流式响应
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<ResponseChoice>,
    pub usage: Option<Usage>,
    pub created: u64,
}

/// 流式响应的单个 chunk
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamChunk {
    pub id: String,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    /// 通常只在最后一个 chunk 出现
    pub usage: Option<Usage>,
}
