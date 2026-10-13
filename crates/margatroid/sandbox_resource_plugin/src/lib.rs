pub use margatroid_types::resource_components::SandboxResource;
mod files;
mod handler;
mod system;
mod types;
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
pub use margatroid_types::resource_events::{
    LoadSandboxResource, LoadSandboxResourceResult, ResourceLoadError, ResourceLoadErrorKind,
};
use resource_id_plugin::{ResourceSet, WorldResourceExt};
use std::path::PathBuf;
pub use types::SandboxResourceLoadLimits;
pub type SandboxResourceLoadError = ResourceLoadError;
pub type SandboxResourceLoadErrorKind = ResourceLoadErrorKind;
use types::SandboxResourceLoadConfig;
pub struct SandboxResourcePlugin {
    root: PathBuf,
    schedule: String,
    limits: SandboxResourceLoadLimits,
}
impl SandboxResourcePlugin {
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
    pub fn with_limits(mut self, limits: SandboxResourceLoadLimits) -> Self {
        self.limits = limits;
        self
    }
}
impl Plugin for SandboxResourcePlugin {
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
            !app.world().contains_resource::<SandboxResourceLoadConfig>(),
            "SandboxResourcePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "resource schedule does not exist"
        );
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut()
            .register_resources::<ResourceSet<SandboxResource>>("sandbox")
            .expect("resource registration failed");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(SandboxResourceLoadConfig {
            root: self.root,
            schedule: self.schedule,
            limits: self.limits,
        });
        app.add_system(&schedule, system::load_sandbox_resource_system);
    }
}
