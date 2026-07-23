use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionDescription {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestTool {
    pub r#type: String, // "function"
    pub function: FunctionDescription,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolChoiceFunction {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolChoiceObject {
    pub r#type: String, // "function"
    pub function: ToolChoiceFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum RequestToolChoice {
    /// "none" | "auto" | "required"
    String(String),
    Object(ToolChoiceObject),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseFunctionCall {
    pub name: String,
    /// JSON 字符串，需要调用方自行解析
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseToolCall {
    pub id: String,
    pub r#type: String, // "function"
    pub function: ResponseFunctionCall,
}

/// streaming delta 里 tool_calls 的单个元素
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}
