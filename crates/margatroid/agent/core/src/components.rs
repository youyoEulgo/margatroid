use core_plugin::Component;

#[derive(Debug)]
pub struct Agent {
    pub(crate) name: String,
    pub(crate) workspace: core_plugin::Entity,
    pub(crate) state: crate::AgentState,
    pub(crate) turn: Option<String>,
    pub(crate) fault: Option<crate::AgentError>,
    pub(crate) updates: crate::AgentUpdateContext,
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
            name,
            workspace,
            state,
            turn,
            source_alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            fault: None,
            updates,
        }
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
pub struct AgentDriverSource {
    pub address: String,
    pub manifest_path: std::path::PathBuf,
}
impl Component for AgentDriverSource {}

impl Drop for Agent {
    fn drop(&mut self) {
        self.close_source();
    }
}
