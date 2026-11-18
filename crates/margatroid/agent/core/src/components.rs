use core_plugin::Component;

#[derive(Debug)]
pub struct Agent {
    pub(crate) name: String,
    pub(crate) workspace: core_plugin::Entity,
    pub(crate) state: crate::AgentState,
    pub(crate) turn: Option<String>,
    pub(crate) fault: Option<crate::AgentError>,
    pub(crate) updates: crate::AgentUpdateContext,
    removed: tokio::sync::watch::Sender<bool>,
    source_alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Agent {
    pub fn new(
        name: String,
        workspace: core_plugin::Entity,
        state: crate::AgentState,
        turn: Option<String>,
        updates: crate::AgentUpdateContext,
    ) -> Self {
        Self {
            removed: tokio::sync::watch::channel(false).0,
            name,
            workspace,
            state,
            turn,
            source_alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            fault: None,
            updates,
        }
    }
    pub fn removal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.removed.subscribe()
    }
    pub fn name(&self) -> &String {
        &self.name
    }
    pub fn workspace(&self) -> core_plugin::Entity {
        self.workspace
    }
    pub fn state(&self) -> crate::AgentState {
        self.state
    }
    pub fn turn(&self) -> &Option<String> {
        &self.turn
    }
    pub fn fault(&self) -> Option<&crate::AgentError> {
        self.fault.as_ref()
    }
    pub fn source_lease(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.source_alive.clone()
    }
    pub(crate) fn reset_source(&mut self) {
        self.source_alive
            .store(false, std::sync::atomic::Ordering::Release);
        self.source_alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    }
    pub(crate) fn close_source(&self) {
        self.source_alive
            .store(false, std::sync::atomic::Ordering::Release);
    }
    pub fn updates(&self) -> crate::AgentUpdateContext {
        self.updates.clone()
    }
}
impl Component for Agent {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResources {
    pub parameters: crate::AgentStartParameters,
}
impl Component for AgentResources {}

impl Drop for Agent {
    fn drop(&mut self) {
        self.close_source();
        self.removed.send_replace(true);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreator {
    pub address: String,
    pub task_id: String,
}
/// Runtime task ownership and its immutable public creation reference.
pub struct AgentTaskBinding {
    creator: AgentCreator,
    _guard: Option<Box<dyn Send + Sync>>,
}
impl AgentTaskBinding {
    pub fn reserve(
        world: &mut core_plugin::World,
        entity: core_plugin::Entity,
        creator: AgentCreator,
    ) {
        world.insert_component(
            entity,
            Self {
                creator,
                _guard: None,
            },
        );
    }
    pub fn install(
        world: &mut core_plugin::World,
        entity: core_plugin::Entity,
        creator: AgentCreator,
        guard: Box<dyn Send + Sync>,
    ) -> Result<(), crate::AgentError> {
        let updates = world
            .get_component::<Agent>(entity)
            .ok_or_else(|| {
                crate::AgentError::new(
                    crate::AgentErrorKind::AgentMissing,
                    "Agent task binding target missing",
                )
            })?
            .updates();
        let binding = world.get_component_mut::<Self>(entity).ok_or_else(|| {
            crate::AgentError::new(
                crate::AgentErrorKind::InvalidRequest,
                "Agent creator was not reserved",
            )
        })?;
        if binding.creator != creator {
            return Err(crate::AgentError::new(
                crate::AgentErrorKind::InvalidRequest,
                "Agent creator task mismatch",
            ));
        }
        binding._guard = Some(guard);
        updates.notify(world, entity, crate::AgentUpdateKind::Changed);
        Ok(())
    }
    pub fn creator(&self) -> &AgentCreator {
        &self.creator
    }
}
impl Component for AgentTaskBinding {}

#[derive(Clone, Debug)]
pub struct AgentSession(pub String);
impl AgentSession {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(format!(
            "agent-session:{}:{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
}
impl Default for AgentSession {
    fn default() -> Self {
        Self::new()
    }
}
impl Component for AgentSession {}
