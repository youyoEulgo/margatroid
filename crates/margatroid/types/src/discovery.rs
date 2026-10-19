//! Read-only directory and installed-driver discovery contracts.
use core_plugin::{Event, Resource};
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryError {
    pub path: PathBuf,
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDriverCatalogSnapshot {
    pub drivers: Vec<ResourceID>,
    pub errors: Vec<DiscoveryError>,
}
#[derive(Clone, Debug, Default)]
pub struct AgentDriverDiscoverySnapshot(pub AgentDriverCatalogSnapshot);
impl Resource for AgentDriverDiscoverySnapshot {}
#[derive(Clone, Debug)]
pub struct QueryAgentDriverCatalog {
    pub id: String,
}
impl Event for QueryAgentDriverCatalog {}
#[derive(Clone, Debug)]
pub struct RefreshAgentDriverCatalog {
    pub id: String,
}
impl Event for RefreshAgentDriverCatalog {}
#[derive(Clone, Debug)]
pub struct AgentDriverCatalogResult {
    pub id: String,
    pub result: Result<AgentDriverCatalogSnapshot, String>,
}
impl Event for AgentDriverCatalogResult {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryEntry {
    pub name: String,
    pub path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryPage {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub directories: Vec<ProjectDirectoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
/// Blocking host-filesystem capability; callers dispatch it outside the ECS thread.
pub trait ProjectDirectoryBrowser: Send + Sync + 'static {
    fn query(
        &self,
        path: Option<PathBuf>,
        cursor: Option<String>,
    ) -> Result<ProjectDirectoryPage, String>;
}
#[derive(Clone)]
pub struct ProjectDirectoryBrowserHandle(pub Arc<dyn ProjectDirectoryBrowser>);
impl Resource for ProjectDirectoryBrowserHandle {}
#[derive(Clone, Debug)]
pub struct QueryProjectDirectories {
    pub id: String,
    pub path: Option<PathBuf>,
    pub cursor: Option<String>,
}
impl Event for QueryProjectDirectories {}
#[derive(Clone, Debug)]
pub struct QueryProjectDirectoriesResult {
    pub id: String,
    pub result: Result<ProjectDirectoryPage, String>,
}
impl Event for QueryProjectDirectoriesResult {}
