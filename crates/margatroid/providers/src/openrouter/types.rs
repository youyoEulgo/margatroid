#![allow(dead_code)]

//! OpenRouter API wire format
//! 所有类型仅在本模块内部使用，不对外暴露
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── 请求专属字段 ──────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestPlugin {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub additional_options: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestPrediction {
    pub r#type: String, // "content"
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Fallback,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum DataCollection {
    Allow,
    Deny,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderSortObject {
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ProviderSort {
    String(String),
    Object(ProviderSortObject),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PercentileThreshold {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p75: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p99: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum PercentileOrNumber {
    Number(f32),
    Percentile(PercentileThreshold),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MaxPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ProviderPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<DataCollection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<ProviderSort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<MaxPrice>,
}

// ── Wire format 完整结构 ───────────────────────────────────

#[derive(Debug, Serialize, Clone, Default)]
pub struct WireRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    // OpenRouter 专属
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<RequestPlugin>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prediction: Option<RequestPrediction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderPreferences>,
}

// ── 响应专属字段 ──────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct ChoiceError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireNonStreamingChoice {
    pub finish_reason: Option<String>,
    pub native_finish_reason: Option<String>,
    pub message: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ChoiceError>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireStreamingChoice {
    pub finish_reason: Option<String>,
    pub native_finish_reason: Option<String>,
    pub delta: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ChoiceError>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CostDetails {
    pub upstream_inference_prompt_cost: f64,
    pub upstream_inference_completions_cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_inference_cost: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerToolUse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search_requests: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_details: Option<CostDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUse>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WireResponse {
    pub id: String,
    pub model: String,
    pub created: u64,
    pub object: String,
    pub choices: Vec<serde_json::Value>, // 延迟解析，convert.rs 负责分辨
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<WireUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
}

// ── 错误 ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiErrorBody {
    pub error: ApiError,
}
