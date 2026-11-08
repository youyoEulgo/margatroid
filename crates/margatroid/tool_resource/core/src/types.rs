use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolEntry {
    pub path: PathBuf,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub owner_task_id: Option<String>,
    pub binding: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InjectedToolDefinition {
    pub alias: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub owner_task_id: Option<String>,
    pub binding: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolInput {
    pub description: String,
    pub input_schema: serde_json::Value,
    pub program: String,
    pub origin: PathBuf,
}
