use crate::{AgentError, AgentStartParameters};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type AgentOperationReply = Arc<Mutex<Option<oneshot::Sender<Result<Entity, AgentError>>>>>;

#[derive(Clone, Debug)]
pub struct StartAgent {
    pub id: String,
    pub workspace: Entity,
    pub name: String,
    pub parameters: AgentStartParameters,
    pub reply: AgentOperationReply,
}
impl Event for StartAgent {}

#[derive(Clone, Debug)]
pub struct RestartAgent {
    pub id: String,
    pub workspace: Entity,
    pub name: String,
    pub parameters: AgentStartParameters,
    pub reply: AgentOperationReply,
}
impl Event for RestartAgent {}

#[derive(Clone, Debug)]
pub struct StopAgent {
    pub id: String,
    pub agent: Entity,
    pub reply: AgentOperationReply,
}
impl Event for StopAgent {}

#[derive(Clone, Debug)]
pub struct AgentException {
    pub id: String,
    pub agent: Entity,
    pub error: AgentError,
}
impl Event for AgentException {}
