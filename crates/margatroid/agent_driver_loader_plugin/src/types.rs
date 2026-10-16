use core_plugin::Resource;
use margatroid_types::resource_components::AgentDriverModelConfig;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct AgentDriverLoadLimits {
    pub max_manifest_bytes: u64,
    pub max_script_bytes: u64,
    pub max_model_id_bytes: usize,
    pub max_stop_sequences: usize,
    pub max_stop_sequence_bytes: usize,
}
impl Default for AgentDriverLoadLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 64 * 1024,
            max_script_bytes: 4 * 1024 * 1024,
            max_model_id_bytes: 1024,
            max_stop_sequences: 128,
            max_stop_sequence_bytes: 4096,
        }
    }
}
#[derive(Clone)]
pub(crate) struct AgentDriverLoadConfig {
    pub root: PathBuf,
    pub schedule: String,
    pub limits: AgentDriverLoadLimits,
}
impl Resource for AgentDriverLoadConfig {}
#[derive(Deserialize)]
pub(crate) struct AgentDriverManifest {
    pub schema_version: u32,
    pub inference: AgentDriverModelConfig,
    #[serde(default)]
    pub dependencies: Vec<AgentDriverDependencyDocument>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentDriverDependencyDocument {
    pub id: String,
}
