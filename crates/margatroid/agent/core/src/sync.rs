use crate::{AgentError, AgentState};
use core_plugin::{Entity, Resource, World};
use std::{future::Future, pin::Pin, sync::Arc};

pub type AgentObservationWait = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentUpdateKind {
    Changed,
    RuntimeInstalled,
    RuntimeStopped,
    Removed,
}
type AgentPublisher =
    Arc<dyn Fn(&mut World, Entity, AgentUpdateKind) -> Vec<AgentObservationWait> + Send + Sync>;
#[derive(Clone)]
pub struct AgentUpdateContext {
    publish: AgentPublisher,
    public: Arc<dyn Fn(Entity, AgentPublicUpdate) + Send + Sync>,
    scope: Arc<std::sync::atomic::AtomicBool>,
    scopes: Arc<
        std::sync::Mutex<std::collections::BTreeMap<Entity, Arc<std::sync::atomic::AtomicBool>>>,
    >,
}
impl Resource for AgentUpdateContext {}
impl std::fmt::Debug for AgentUpdateContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentUpdateContext").finish_non_exhaustive()
    }
}
impl AgentUpdateContext {
    pub fn new(
        publish: impl Fn(&mut World, Entity, AgentUpdateKind) -> Vec<AgentObservationWait>
            + Send
            + Sync
            + 'static,
        public: impl Fn(Entity, AgentPublicUpdate) + Send + Sync + 'static,
    ) -> Self {
        Self {
            publish: Arc::new(publish),
            public: Arc::new(public),
            scope: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            scopes: Arc::default(),
        }
    }
    pub fn scoped(&self, owner: Entity) -> Self {
        let mut scopes = self.scopes.lock().unwrap();
        let scope = scopes
            .entry(owner)
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(true)))
            .clone();
        Self {
            publish: self.publish.clone(),
            public: self.public.clone(),
            scope,
            scopes: self.scopes.clone(),
        }
    }
    pub fn update_public(&self, owner: Entity, value: AgentPublicUpdate) {
        if !self.scope.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        (self.public)(owner, value)
    }
    pub fn notify(
        &self,
        world: &mut World,
        owner: Entity,
        kind: AgentUpdateKind,
    ) -> Vec<AgentObservationWait> {
        if matches!(
            kind,
            AgentUpdateKind::RuntimeStopped | AgentUpdateKind::Removed
        ) {
            if let Some(scope) = self.scopes.lock().unwrap().remove(&owner) {
                scope.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        (self.publish)(world, owner, kind)
    }
}
pub enum AgentChange {
    Transition {
        state: AgentState,
        turn: Option<String>,
    },
    Fault(AgentError),
}
pub fn update_agent(
    world: &mut World,
    owner: Entity,
    context: &AgentUpdateContext,
    change: AgentChange,
) -> Result<(), AgentError> {
    let agent = world
        .get_component_mut::<crate::Agent>(owner)
        .ok_or_else(|| AgentError::new(crate::AgentErrorKind::AgentMissing, "Agent missing"))?;
    let changed = match change {
        AgentChange::Transition { state, turn } => {
            let changed = agent.state != state
                || agent.turn != turn
                || (state == AgentState::Idle && agent.fault.is_some());
            if state == AgentState::Idle {
                agent.fault = None;
            }
            agent.state = state;
            agent.turn = turn;
            changed
        }
        AgentChange::Fault(error) => {
            let changed = agent.fault.as_ref() != Some(&error)
                || agent.state != AgentState::Stopping
                || agent.turn.is_some();
            agent.fault = Some(error);
            agent.state = AgentState::Stopping;
            agent.turn = None;
            changed
        }
    };
    if changed {
        let waits = context.notify(world, owner, AgentUpdateKind::Changed);
        assert!(
            waits.is_empty(),
            "state publication cannot create monitor cleanup work"
        );
    }
    Ok(())
}
pub fn notify_agent_runtime(
    world: &mut World,
    owner: Entity,
    stopped: bool,
) -> Result<Vec<AgentObservationWait>, AgentError> {
    let agent = world
        .get_component_mut::<crate::Agent>(owner)
        .ok_or_else(|| AgentError::new(crate::AgentErrorKind::AgentMissing, "Agent missing"))?;
    if stopped {
        agent.close_source();
    } else {
        agent.reset_source();
    }
    let context = world
        .get_component::<crate::Agent>(owner)
        .ok_or_else(|| AgentError::new(crate::AgentErrorKind::AgentMissing, "Agent missing"))?
        .updates
        .clone();
    Ok(context.notify(
        world,
        owner,
        if stopped {
            AgentUpdateKind::RuntimeStopped
        } else {
            AgentUpdateKind::RuntimeInstalled
        },
    ))
}
pub fn remove_agent(world: &mut World, owner: Entity) {
    let context = world
        .get_component::<crate::Agent>(owner)
        .map(|agent| agent.updates.clone());
    world.despawn(owner);
    if let Some(context) = context {
        let waits = context.notify(world, owner, AgentUpdateKind::Removed);
        assert!(
            waits.is_empty(),
            "Agent must await actual monitor cleanup before despawn"
        );
    }
}

#[derive(Clone, Debug)]
pub enum AgentPublicUpdate {
    Tools(Vec<PublicToolInfo>),
    Sandboxes(Vec<(String, std::path::PathBuf)>),
    Sandbox(Option<String>),
    Model(Option<PublicModelInfo>),
}
#[derive(Clone, Debug)]
pub struct PublicToolInfo {
    pub alias: String,
    pub path: std::path::PathBuf,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub owner_task_id: Option<String>,
    pub binding: Option<serde_json::Value>,
}
#[derive(Clone, Debug)]
pub struct PublicModelInfo {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub api_type: String,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}
