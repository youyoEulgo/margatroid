use serde::{Deserialize, Serialize};

use crate::{
    AgentId, AgentImageReference, ResourceReference, WorkspaceBundle, WorkspaceId, WorkspaceName,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAgentSpec {
    pub id: AgentId,
    pub image: AgentImageReference,
    #[serde(default)]
    pub skills: Vec<ResourceReference>,
    #[serde(default)]
    pub workflows: Vec<ResourceReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_volume: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub name: WorkspaceName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub manager: AgentId,
    #[serde(default)]
    pub agents: Vec<WorkspaceAgentSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Created,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: WorkspaceName,
    pub status: WorkspaceStatus,
    pub agent_count: u32,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub bundle: WorkspaceBundle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceResponse {
    pub workspace: WorkspaceSummary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWorkspacesResponse {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSummary>,
}
