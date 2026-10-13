mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
pub use margatroid_types::agent_image_events::{
    ImageLoadError, LoadAgentImage, LoadAgentImageResult,
};
pub use margatroid_types::resource_components::{AgentImage, AgentImageModelConfig};
use resource_id_plugin::{ResourceSet, WorldResourceExt};
use std::path::PathBuf;
pub use types::AgentImageLoadLimits;
pub use AgentImageLoaderPlugin as AgentImagePlugin;
pub type AgentImageLoadError = ImageLoadError;
use types::AgentImageLoadConfig;

pub struct AgentImageLoaderPlugin {
    root: PathBuf,
    schedule: String,
    limits: AgentImageLoadLimits,
}
impl AgentImageLoaderPlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ImageLoadError> {
        Ok(Self {
            root: handler::normalize_root(root.into())?,
            schedule: RuntimePlugin::PRE_UPDATE.into(),
            limits: Default::default(),
        })
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_limits(mut self, limits: AgentImageLoadLimits) -> Self {
        self.limits = limits;
        self
    }
}
impl Plugin for AgentImageLoaderPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin is not installed"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin is not installed"
        );
        assert!(
            !app.world().contains_resource::<AgentImageLoadConfig>(),
            "AgentImagePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "image schedule does not exist"
        );
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut()
            .register_resources::<ResourceSet<AgentImage>>("image")
            .expect("image resource registration failed");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(AgentImageLoadConfig {
            root: self.root,
            schedule: self.schedule,
            limits: self.limits,
        });
        app.add_system(&schedule, system::load_agent_image_system);
    }
}
#[cfg(test)]
mod tests;
