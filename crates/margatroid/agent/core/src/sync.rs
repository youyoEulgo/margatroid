use crate::{AgentError, AgentUpdateKind};
use core_plugin::{Entity, Resource, World};
use std::{future::Future, pin::Pin, sync::Arc};

pub type AgentObservationWait = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>;
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
            AgentUpdateKind::SourcesClosing | AgentUpdateKind::Removed
        ) {
            if let Some(scope) = self.scopes.lock().unwrap().remove(&owner) {
                scope.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        (self.publish)(world, owner, kind)
    }
}
pub fn remove_agent(world: &mut World, owner: Entity) {
    world.despawn(owner);
    let context = world.get_resource::<AgentUpdateContext>().cloned();
    if let Some(context) = context {
        let waits = context.notify(world, owner, AgentUpdateKind::Removed);
        assert!(
            waits.is_empty(),
            "removed observation cannot require cleanup"
        );
    }
}
pub(crate) fn publish_changed(world: &mut World, owner: Entity) {
    if let Some(context) = world.get_resource::<AgentUpdateContext>().cloned() {
        let waits = context.notify(world, owner, AgentUpdateKind::Changed);
        assert!(waits.is_empty(), "state updates cannot require cleanup");
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
