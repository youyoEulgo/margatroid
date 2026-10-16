use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverLoadError {
    pub message: String,
}
impl DriverLoadError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
impl std::fmt::Display for DriverLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for DriverLoadError {}

#[derive(Clone, Debug)]
pub struct LoadAgentDriver {
    pub id: String,
    pub target: Entity,
    pub resource_id: ResourceID,
}
impl Event for LoadAgentDriver {}
#[derive(Clone, Debug)]
pub struct LoadAgentDriverResult {
    pub id: String,
    pub target: Entity,
    pub resource_id: ResourceID,
    pub result: Result<(), DriverLoadError>,
}
impl Event for LoadAgentDriverResult {}

#[derive(Clone)]
pub struct AgentDriverReadOutput {
    pub request: LoadAgentDriver,
    pub result: Result<crate::resource_components::AgentDriver, DriverLoadError>,
    pub completed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub supervision: std::sync::Arc<dyn Send + Sync>,
}
