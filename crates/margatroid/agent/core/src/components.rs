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
