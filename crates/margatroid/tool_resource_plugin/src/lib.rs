pub use margatroid_types::resource_components::ToolResource;
mod files;
mod handler;
mod system;
mod types;
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
pub use margatroid_types::resource_events::{
    LoadToolResource, LoadToolResourceResult, ResourceLoadError, ResourceLoadErrorKind,
};
use resource_id_plugin::{ResourceSet, WorldResourceExt};
use std::path::PathBuf;
pub use types::ToolResourceLoadLimits;
pub type ToolResourceLoadError = ResourceLoadError;
pub type ToolResourceLoadErrorKind = ResourceLoadErrorKind;
use types::ToolResourceLoadConfig;
pub struct ToolResourcePlugin {
    root: PathBuf,
    schedule: String,
    limits: ToolResourceLoadLimits,
}
impl ToolResourcePlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ResourceLoadError> {
        Ok(Self {
            root: files::normalize_root(root.into())?,
            schedule: RuntimePlugin::PRE_UPDATE.into(),
            limits: Default::default(),
        })
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_limits(mut self, limits: ToolResourceLoadLimits) -> Self {
        self.limits = limits;
        self
    }
}
impl Plugin for ToolResourcePlugin {
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
            !app.world().contains_resource::<ToolResourceLoadConfig>(),
            "ToolResourcePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "resource schedule does not exist"
        );
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut()
            .register_resources::<ResourceSet<ToolResource>>("tool")
            .expect("resource registration failed");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(ToolResourceLoadConfig {
            root: self.root,
            schedule: self.schedule,
            limits: self.limits,
        });
        app.add_system(&schedule, system::load_tool_resource_system);
    }
}
