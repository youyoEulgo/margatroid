use core_plugin::{Entity, Event};
use margatroid_types::{AgentError, AgentErrorKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFailureKind {
    InvalidRequest,
    AgentMissing,
    DuplicateAgent,
    LuaRuntime,
    Mcl,
    Import,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateResult {
    pub id: String,
    pub result: Result<Entity, AgentError>,
}

impl Event for AgentCreateResult {}

pub fn failure(kind: AgentFailureKind, message: impl Into<String>) -> AgentError {
    let kind = match kind {
        AgentFailureKind::InvalidRequest => AgentErrorKind::InvalidRequest,
        AgentFailureKind::AgentMissing => AgentErrorKind::AgentMissing,
        AgentFailureKind::DuplicateAgent => AgentErrorKind::DuplicateAgent,
        AgentFailureKind::LuaRuntime => AgentErrorKind::LuaRuntime,
        AgentFailureKind::Mcl => AgentErrorKind::Mcl,
        AgentFailureKind::Import => AgentErrorKind::Import,
        AgentFailureKind::Stopped => AgentErrorKind::Stopped,
    };
    AgentError::new(kind, message)
}
