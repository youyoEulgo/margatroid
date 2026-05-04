use crate::{
    message::RequestMessage,
    tool::{RequestTool, RequestToolChoice},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FormatJsonSchema {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    pub schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseFormat {
    pub r#type: String, // "json_object" | "json_schema" | "text"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<FormatJsonSchema>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum RequestStop {
    Single(String),
    Multiple(Vec<String>),
}

/// 上层代码使用的统一请求结构
/// 不包含任何 provider 专属字段
/// provider 专属行为通过 provider_options 传递
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatRequest {
    pub messages: Vec<RequestMessage>,

    /// 模型 ID，格式由各 provider 自行解释
    pub model: String,

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
    pub stop: Option<RequestStop>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<RequestTool>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<RequestToolChoice>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<HashMap<i64, f32>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// provider 专属参数，不进入通用类型系统
    /// 例如 OpenRouter 的 provider preferences
    #[serde(skip)]
    pub provider_options: HashMap<String, serde_json::Value>,
}
