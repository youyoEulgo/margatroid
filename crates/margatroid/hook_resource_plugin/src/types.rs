use core_plugin::Resource;
use std::path::PathBuf;
#[derive(Clone)]
pub(crate) struct HookResourceLoadConfig {
    pub root: PathBuf,
    pub schedule: String,
    pub limits: HookResourceLoadLimits,
}
impl Resource for HookResourceLoadConfig {}
#[derive(Clone, Copy, Debug)]
pub struct HookResourceLoadLimits {
    pub max_definition_bytes: u64,
    pub max_content_bytes: u64,
    pub max_script_bytes: u64,
    pub max_policy_bytes: u64,
}
impl Default for HookResourceLoadLimits {
    fn default() -> Self {
        Self {
            max_definition_bytes: 1024 * 1024,
            max_content_bytes: 8 * 1024 * 1024,
            max_script_bytes: 8 * 1024 * 1024,
            max_policy_bytes: 8 * 1024 * 1024,
        }
    }
}
