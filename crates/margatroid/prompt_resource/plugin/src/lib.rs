mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin};
use prompt_resource_core::PromptReadLimits;

use types::ImportConfig;

#[derive(Clone, Debug)]
pub struct PromptResourcePlugin {
    schedule: String,
    limits: PromptReadLimits,
}

impl PromptResourcePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            limits: PromptReadLimits::default(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_limits(mut self, limits: PromptReadLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl Default for PromptResourcePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for PromptResourcePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin is required by PromptResourcePlugin"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin is required by PromptResourcePlugin"
        );
        assert!(
            !app.world().contains_resource::<ImportConfig>(),
            "PromptResourcePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "PromptResourcePlugin schedule does not exist"
        );
        assert!(
            self.schedule != RuntimePlugin::STARTUP,
            "PromptResourcePlugin requires a recurring schedule"
        );
        app.world_mut().insert_resource(ImportConfig {
            limits: self.limits,
        });
        app.add_system(&self.schedule, system::prompt_resource_system);
    }
}
