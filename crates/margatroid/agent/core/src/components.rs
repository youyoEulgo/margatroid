use crate::AgentState;
use core_plugin::{Component, Entity};

#[derive(Clone, Debug)]
pub struct Agent {
    pub name: String,
    pub workspace: Entity,
    pub state: AgentState,
    pub turn: Option<String>,
}

impl Component for Agent {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDriverSource {
    pub address: String,
    pub manifest_path: std::path::PathBuf,
}
impl Component for AgentDriverSource {}
