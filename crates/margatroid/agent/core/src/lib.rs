mod components;
mod error;
mod events;
mod sync;
mod types;
pub use sync::{
    notify_agent_runtime, remove_agent, update_agent, AgentChange, AgentObservationWait,
    AgentUpdateContext, AgentUpdateKind,
};

pub use components::{Agent, AgentResources};
pub use error::{AgentError, AgentErrorKind};
pub use events::{AgentException, AgentOperationReply, RestartAgent, StartAgent, StopAgent};
pub use types::{AgentResourceRoot, AgentStartParameters, AgentState};

use core_plugin::{Entity, World};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

pub trait WorldAgentExt {
    fn agent_state(&self, agent: Entity) -> Result<&AgentState, AgentError>;
    fn agent_transition(
        &mut self,
        agent: Entity,
        state: AgentState,
        turn: Option<String>,
    ) -> Result<(), AgentError>;
    fn agent_ready(&mut self, agent: Entity) -> Result<(), AgentError>;
    fn agent_start_turn(&mut self, agent: Entity) -> Result<String, AgentError>;
    fn agent_finish_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>;
    fn agent_abort_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>;
}

impl WorldAgentExt for World {
    fn agent_state(&self, entity: Entity) -> Result<&AgentState, AgentError> {
        self.get_component::<Agent>(entity)
            .map(|agent| &agent.state)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "agent does not exist"))
    }

    fn agent_transition(
        &mut self,
        owner: Entity,
        state: AgentState,
        turn: Option<String>,
    ) -> Result<(), AgentError> {
        let context = self
            .get_component::<Agent>(owner)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "Agent missing"))?
            .updates
            .clone();
        update_agent(
            self,
            owner,
            &context,
            AgentChange::Transition { state, turn },
        )
    }
    fn agent_ready(&mut self, entity: Entity) -> Result<(), AgentError> {
        let agent = self
            .get_component::<Agent>(entity)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "Agent missing"))?;
        if !matches!(agent.state, AgentState::Starting | AgentState::Running)
            || agent.turn.is_some()
        {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "Agent cannot become ready in current state",
            ));
        }
        self.agent_transition(entity, AgentState::Running, None)
    }
    fn agent_start_turn(&mut self, entity: Entity) -> Result<String, AgentError> {
        let agent = self
            .get_component_mut::<Agent>(entity)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "agent does not exist"))?;
        if agent.state != AgentState::Running {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "a running Agent is required",
            ));
        }
        if let Some(turn) = &agent.turn {
            return Ok(turn.clone());
        }
        static PREFIX: OnceLock<String> = OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let prefix = PREFIX.get_or_init(|| {
            format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock must follow Unix epoch")
                    .as_nanos()
            )
        });
        let turn = format!("turn:{prefix}:{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let context = agent.updates.clone();
        update_agent(
            self,
            entity,
            &context,
            AgentChange::Transition {
                state: AgentState::Running,
                turn: Some(turn.clone()),
            },
        )?;
        Ok(turn)
    }

    fn agent_finish_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError> {
        let agent = self
            .get_component_mut::<Agent>(entity)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "agent does not exist"))?;
        if turn.is_empty()
            || agent.state != AgentState::Running
            || agent.turn.as_deref().is_some_and(|current| current != turn)
        {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn does not match the running Agent",
            ));
        }
        let context = agent.updates.clone();
        update_agent(
            self,
            entity,
            &context,
            AgentChange::Transition {
                state: AgentState::Running,
                turn: None,
            },
        )?;
        Ok(())
    }

    fn agent_abort_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError> {
        let agent = self
            .get_component_mut::<Agent>(entity)
            .ok_or_else(|| AgentError::new(AgentErrorKind::AgentMissing, "agent does not exist"))?;
        if turn.is_empty()
            || agent.state != AgentState::Running
            || agent.turn.as_deref() != Some(turn)
        {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn does not match the running Agent",
            ));
        }
        let context = agent.updates.clone();
        update_agent(
            self,
            entity,
            &context,
            AgentChange::Transition {
                state: AgentState::Running,
                turn: None,
            },
        )?;
        Ok(())
    }
}

pub use sync::{AgentPublicUpdate, PublicModelInfo, PublicToolInfo};

pub use components::AgentOwner;

pub use components::AgentSession;
