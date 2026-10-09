mod access;
mod error;
mod handler;
mod types;

use core_plugin::{App, Component, Plugin, Resource};
use library_component_plugin::LibraryComponent;
use resource_id_plugin::ResourceIdPluginInstalled;
use std::path::PathBuf;
use std::sync::Arc;

pub use access::{
    find_resource, hook_resource, prompt_resource, read_skill_content, read_tool_script,
    sandbox_resource, skill_resource, tool_definition, tool_resource, ExecutableDefinition,
};
pub use error::ResourceLibraryError;
pub use handler::{install_resource, release_resource_owner};
pub use types::{ResourceDefinition, ResourceOwners};

#[derive(Clone, Debug)]
pub struct ToolResource {
    pub description: String,
    pub input_schema: serde_json::Value,
    pub script_path: PathBuf,
}
impl Component for ToolResource {}
impl LibraryComponent for ToolResource {}

#[derive(Clone, Debug)]
pub struct SkillResource {
    pub description: String,
    pub content_path: PathBuf,
}
impl Component for SkillResource {}
impl LibraryComponent for SkillResource {}

#[derive(Clone, Debug)]
pub struct PromptResource {
    pub role: String,
    pub content: Arc<str>,
}
impl Component for PromptResource {}
impl LibraryComponent for PromptResource {}

#[derive(Clone, Debug)]
pub struct SandboxResource {
    pub policy: Arc<str>,
}
impl Component for SandboxResource {}
impl LibraryComponent for SandboxResource {}

#[derive(Clone, Debug)]
pub struct HookResource {
    pub description: String,
    pub input_schema: serde_json::Value,
}
impl Component for HookResource {}
impl LibraryComponent for HookResource {}

#[derive(Clone, Debug)]
pub struct ResourceLibraryRoot(pub PathBuf);
impl Resource for ResourceLibraryRoot {}

pub struct ResourceLibraryPlugin {
    root: PathBuf,
}

impl ResourceLibraryPlugin {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Plugin for ResourceLibraryPlugin {
    fn build(self, app: &mut App) {
        assert!(app.world().contains_resource::<ResourceIdPluginInstalled>());
        assert!(!app.world().contains_resource::<ResourceLibraryRoot>());
        app.world_mut()
            .insert_resource(ResourceLibraryRoot(self.root));
    }
}
