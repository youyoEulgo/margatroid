use core_plugin::{Entity, Event};
use resource_id_plugin::ResourceID;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLoadError {
    pub message: String,
}
impl ImageLoadError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
impl std::fmt::Display for ImageLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ImageLoadError {}

#[derive(Clone, Debug)]
pub struct LoadAgentImage {
    pub id: String,
    pub target: Entity,
    pub resource_id: ResourceID,
}
impl Event for LoadAgentImage {}
#[derive(Clone, Debug)]
pub struct LoadAgentImageResult {
    pub id: String,
    pub target: Entity,
    pub resource_id: ResourceID,
    pub result: Result<(), ImageLoadError>,
}
impl Event for LoadAgentImageResult {}

#[derive(Clone)]
pub struct AgentImageReadOutput {
    pub request: LoadAgentImage,
    pub result: Result<crate::resource_components::AgentImage, ImageLoadError>,
    pub completed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub supervision: std::sync::Arc<dyn Send + Sync>,
}
