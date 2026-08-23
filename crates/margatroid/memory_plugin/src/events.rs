use core_plugin::{Entity, Event};

use crate::error::MemoryError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMemoryWriteFailed {
    pub agent: Entity,
    pub error: MemoryError,
}

impl Event for AgentMemoryWriteFailed {}
