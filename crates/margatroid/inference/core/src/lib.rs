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
    pub entries: BTreeMap<String, ModelRoute>,
}
impl ModelRoutes {
    pub fn get(&self, id: &str) -> Option<&ModelRoute> {
        self.entries.get(id)
    }
}
impl Resource for ModelRoutes {}
