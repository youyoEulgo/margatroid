mod components;
mod error;
mod events;
mod sync;
mod types;
pub use components::Agent;
use core_plugin::{Entity, World};
pub use error::{AgentError, AgentErrorKind};
pub use events::{AgentOperationReply, StartAgent, StopAgent};
pub use sync::{
    remove_agent, AgentObservationWait, AgentPublicUpdate, AgentUpdateContext, PublicModelInfo,
    PublicToolInfo,
};
pub use types::{
    AgentCloseFuture, AgentHandle, AgentReadyState, AgentReply, AgentState, AgentUpdateKind,
    AgentWakeSlot,
};

pub(crate) fn unique_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    static EPOCH: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    });
    format!(
        "{prefix}:{}:{epoch}:{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
pub trait WorldAgentExt {
    fn agent_state(&self, entity: Entity) -> Result<&AgentState, AgentError>;
    fn agent_ready(&mut self, entity: Entity) -> Result<(), AgentError>;
    fn agent_start_turn(&mut self, entity: Entity) -> Result<String, AgentError>;
    fn agent_finish_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError>;
    fn agent_abort_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError>;
    fn agent_set_turn(&mut self, entity: Entity, turn: Option<String>) -> Result<(), AgentError>;
    fn agent_begin_close(&mut self, entity: Entity) -> Result<(), AgentError>;
}
fn missing() -> AgentError {
    AgentError::new(AgentErrorKind::AgentMissing, "Agent entity missing")
}
impl WorldAgentExt for World {
    fn agent_state(&self, e: Entity) -> Result<&AgentState, AgentError> {
        self.get_component::<Agent>(e)
            .map(|a| &a.state)
            .ok_or_else(missing)
    }
    fn agent_ready(&mut self, e: Entity) -> Result<(), AgentError> {
        let changed = self
            .get_component_mut::<Agent>(e)
            .ok_or_else(missing)?
            .mark_ready()?;
        if changed {
            sync::publish_changed(self, e);
        }
        Ok(())
    }
    fn agent_set_turn(&mut self, e: Entity, turn: Option<String>) -> Result<(), AgentError> {
        let changed = self
            .get_component_mut::<Agent>(e)
            .ok_or_else(missing)?
            .set_turn(turn)?;
        if changed {
            sync::publish_changed(self, e);
        }
        Ok(())
    }
    fn agent_start_turn(&mut self, e: Entity) -> Result<String, AgentError> {
        let a = self.get_component::<Agent>(e).ok_or_else(missing)?;
        if a.state() != AgentState::Running {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn requires a running Agent",
            ));
        }
        if let Some(turn) = a.turn() {
            return Ok(turn.clone());
        }
        let turn = unique_id("turn");
        self.agent_set_turn(e, Some(turn.clone()))?;
        Ok(turn)
    }
    fn agent_finish_turn(&mut self, e: Entity, turn: &str) -> Result<(), AgentError> {
        let a = self.get_component::<Agent>(e).ok_or_else(missing)?;
        if turn.is_empty()
            || a.state() != AgentState::Running
            || a.turn().as_deref().is_some_and(|current| current != turn)
        {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn does not match current Agent",
            ));
        }
        self.agent_set_turn(e, None)
    }
    fn agent_abort_turn(&mut self, e: Entity, turn: &str) -> Result<(), AgentError> {
        if turn.is_empty()
            || self
                .get_component::<Agent>(e)
                .ok_or_else(missing)?
                .turn()
                .as_deref()
                != Some(turn)
        {
            return Err(AgentError::new(
                AgentErrorKind::StateRejected,
                "turn does not match current Agent",
            ));
        }
        self.agent_set_turn(e, None)
    }
    fn agent_begin_close(&mut self, e: Entity) -> Result<(), AgentError> {
        let changed = self
            .get_component_mut::<Agent>(e)
            .ok_or_else(missing)?
            .begin_close();
        if changed {
            sync::publish_changed(self, e);
        }
        Ok(())
    }
}
