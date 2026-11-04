mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
use tool_resource_core::ToolReadLimits;

use types::ImportConfig;

#[derive(Clone, Debug)]
pub struct ToolResourcePlugin {
    schedule: String,
    limits: ToolReadLimits,
}

impl ToolResourcePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            limits: ToolReadLimits::default(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_limits(mut self, limits: ToolReadLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl Default for ToolResourcePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for ToolResourcePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin is required by ToolResourcePlugin"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin is required by ToolResourcePlugin"
        );
        assert!(
            !app.world().contains_resource::<ImportConfig>(),
            "ToolResourcePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "ToolResourcePlugin schedule does not exist"
        );
        assert!(
            self.schedule != RuntimePlugin::STARTUP,
            "ToolResourcePlugin requires a recurring schedule"
        );
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut().insert_resource(ImportConfig {
            schedule: self.schedule.clone(),
            limits: self.limits,
        });
        app.add_system(&self.schedule, system::tool_resource_system);
    }
}
