use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use core_plugin::{Entity, Resource};
use margatroid_types::AgentError;

use crate::error::WorkspaceError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAgentState {
    Creating,
    Ready { agent: Entity },
    Failed { error: WorkspaceError },
}

impl Resource for WorkspaceRegistry {}

#[derive(Default)]
pub struct WorkspaceRegistry {
    pub agent_images_root: Arc<PathBuf>,
    pub workspaces: Vec<Entity>,
    pub pending_images: BTreeMap<String, (Entity, String)>,
    pub pending_agents: BTreeMap<
        String,
        (
            Entity,
            String,
            tokio::sync::oneshot::Receiver<Result<Entity, AgentError>>,
        ),
    >,
    pub pending_mcl_commands: BTreeMap<
        String,
        (
            std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
            tokio::sync::oneshot::Receiver<
                Result<mcl_plugin::MclCommandValue, mcl_plugin::MclError>,
            >,
        ),
    >,
}
