use std::path::PathBuf;
use std::sync::Arc;

use core_plugin::{Entity, Event};
use margatroid_types::ResourceId;

use crate::error::AgentImageLoadError;
use crate::types::AgentImageLoaderLimits;

pub struct LoadAgentImage {
    pub id: String,
    pub reference: ResourceId,
}

impl Event for LoadAgentImage {}

pub struct LoadAgentImageResult {
    pub id: String,
    pub reference: ResourceId,
    pub result: Result<Entity, AgentImageLoadError>,
}

impl Event for LoadAgentImageResult {}

pub(crate) struct AgentImageReadTask {
    pub(crate) reference: ResourceId,
    pub(crate) root: Arc<PathBuf>,
    pub(crate) limits: AgentImageLoaderLimits,
}

impl Event for AgentImageReadTask {}
