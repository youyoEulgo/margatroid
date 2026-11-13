mod components;
mod error;
mod events;
mod types;
pub use components::{ActiveInference, AgentInferenceConfig};
use core_plugin::Resource;
pub use error::{InferenceError, InferenceErrorKind};
pub use events::{InferenceReply, InferenceRequest, InitInference};
use std::collections::BTreeMap;
pub use types::*;
#[derive(Clone, Default, Debug)]
pub struct ModelRoutes {
    entries: BTreeMap<String, ModelRoute>,
}
impl ModelRoutes {
    pub fn new(entries: BTreeMap<String, ModelRoute>) -> Self {
        Self { entries }
    }
    pub fn replace(
        &mut self,
        entries: BTreeMap<String, ModelRoute>,
        configs: &[AgentInferenceConfig],
    ) {
        self.entries = entries;
        for config in configs {
            config.publish_current(self);
        }
    }
    pub fn entries(&self) -> &BTreeMap<String, ModelRoute> {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModelRoute> {
        self.entries.get(id)
    }
}
impl Resource for ModelRoutes {}

pub use components::PublicModelInfo;
pub type ModelSyncBinding = client_core::ProbeFactory<Option<PublicModelInfo>>;
