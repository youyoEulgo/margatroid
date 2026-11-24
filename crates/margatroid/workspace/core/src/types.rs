use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceState {
    Running,
    Deleting,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceUpdateKind {
    Installed,
    Changed,
    Closing,
    Removed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCategory {
    Normal,
    Temporary,
    Archived,
}
impl AgentCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Temporary => "Temporary",
            Self::Archived => "Archived",
        }
    }
    pub fn directory(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Temporary => "temporary",
            Self::Archived => "archived",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAgent {
    pub parent_node: String,
    pub id: String,
    pub tool_path: PathBuf,
    pub tool_arguments: serde_json::Value,
    pub node_name: Option<String>,
    pub alias: Option<String>,
    pub category: AgentCategory,
    pub directory_name: String,
    pub runtime_name: String,
    pub previous_node_name: Option<String>,
    pub previous_alias: Option<String>,
}
impl WorkspaceAgent {
    pub fn address(&self) -> String {
        format!(
            "{}/{}",
            self.parent_node.trim_end_matches('/'),
            self.runtime_name
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRecordsSnapshot {
    pub revision: u64,
    pub agents: Vec<WorkspaceAgent>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub address: String,
    pub alias: String,
    pub project_root: PathBuf,
}
