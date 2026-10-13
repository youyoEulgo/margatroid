use core_plugin::Resource;
use std::path::PathBuf;
#[derive(Clone)]
pub(crate) struct SkillResourceLoadConfig {
    pub root: PathBuf,
    pub schedule: String,
    pub limits: SkillResourceLoadLimits,
}
impl Resource for SkillResourceLoadConfig {}
#[derive(Clone, Copy, Debug)]
pub struct SkillResourceLoadLimits {
    pub max_definition_bytes: u64,
    pub max_content_bytes: u64,
    pub max_script_bytes: u64,
    pub max_policy_bytes: u64,
}
impl Default for SkillResourceLoadLimits {
    fn default() -> Self {
        Self {
            max_definition_bytes: 1024 * 1024,
            max_content_bytes: 8 * 1024 * 1024,
            max_script_bytes: 8 * 1024 * 1024,
            max_policy_bytes: 8 * 1024 * 1024,
        }
    }
}
