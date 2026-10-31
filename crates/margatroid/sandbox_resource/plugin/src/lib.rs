mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin};
use sandbox_resource_core::SandboxReadLimits;

use types::ImportConfig;

#[derive(Clone, Debug)]
pub struct SandboxResourcePlugin {
    schedule: String,
    limits: SandboxReadLimits,
}

impl SandboxResourcePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            limits: SandboxReadLimits::default(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_limits(mut self, limits: SandboxReadLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl Default for SandboxResourcePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for SandboxResourcePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin is required by SandboxResourcePlugin"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin is required by SandboxResourcePlugin"
        );
        assert!(
            !app.world().contains_resource::<ImportConfig>(),
            "SandboxResourcePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "SandboxResourcePlugin schedule does not exist"
        );
        assert!(
            self.schedule != RuntimePlugin::STARTUP,
            "SandboxResourcePlugin requires a recurring schedule"
        );
        app.world_mut().insert_resource(ImportConfig {
            limits: self.limits,
        });
        app.add_system(&self.schedule, system::sandbox_resource_system);
    }
}
