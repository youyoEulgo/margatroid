use crate::{
    AgentControlKind, AgentControlReply, AgentCreateReply, AgentMemoryHandle, AgentModelInfo,
};
use core_plugin::{Entity, Event};
use lua_runtime_plugin::LuaProgram;
use margatroid_types::{LuaVmId, ResourceId, TokenUsage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct AgentCreateRequest {
    pub id: String,
    pub agent_id: ResourceId,
    pub workspace_id: Entity,
    pub image_entity: Entity,
    pub base_lua: LuaProgram,
    pub project_root: PathBuf,
    pub image_root: PathBuf,
    pub home_root: PathBuf,
    pub model: AgentModelInfo,
    pub memory: AgentMemoryHandle,
    pub token_usage: TokenUsage,
    pub image_dependencies: Arc<[ResourceId]>,
    pub image_sources: HashMap<ResourceId, Arc<str>>,
    pub reply: AgentCreateReply,
}
impl Event for AgentCreateRequest {}
#[derive(Clone, Debug)]
pub struct AgentControl {
    pub id: String,
    pub agent: Entity,
    pub control: AgentControlKind,
    pub reply: AgentControlReply,
}
impl Event for AgentControl {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInitializationCompleted {
    pub agent: Entity,
    pub vm_id: LuaVmId,
}

impl Event for AgentInitializationCompleted {}
