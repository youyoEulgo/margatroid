use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDriverManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub inference: DriverInferenceConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriverInferenceConfig {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentDriverInput {
    pub manifest: AgentDriverManifest,
    pub program: String,
    pub origin: PathBuf,
    pub default_settings: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDriverReadLimits {
    pub manifest_bytes: usize,
    pub program_bytes: usize,
    pub settings_bytes: usize,
}

impl Default for AgentDriverReadLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 64 * 1024,
            program_bytes: 4 * 1024 * 1024,
            settings_bytes: 256 * 1024,
        }
    }
}
