use core_plugin::Resource;
use std::path::PathBuf;
#[derive(Clone)]
pub(crate) struct PromptResourceLoadConfig {
    pub root: PathBuf,
    pub schedule: String,
    pub limits: PromptResourceLoadLimits,
}
impl Resource for PromptResourceLoadConfig {}
#[derive(Clone, Copy, Debug)]
pub struct PromptResourceLoadLimits {
    pub max_definition_bytes: u64,
    pub max_content_bytes: u64,
    pub max_script_bytes: u64,
    pub max_policy_bytes: u64,
}
impl Default for PromptResourceLoadLimits {
    fn default() -> Self {
        Self {
            max_definition_bytes: 1024 * 1024,
            max_content_bytes: 8 * 1024 * 1024,
            max_script_bytes: 8 * 1024 * 1024,
            max_policy_bytes: 8 * 1024 * 1024,
        }
    }
}
