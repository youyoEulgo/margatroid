mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
pub use margatroid_types::agent_driver_events::{
    DriverLoadError, LoadAgentDriver, LoadAgentDriverResult,
};
pub use margatroid_types::resource_components::{AgentDriver, AgentDriverModelConfig};
use resource_id_plugin::{ResourceSet, WorldResourceExt};
use std::path::PathBuf;
pub use types::AgentDriverLoadLimits;
pub use AgentDriverLoaderPlugin as AgentDriverPlugin;
pub type AgentDriverLoadError = DriverLoadError;
use types::AgentDriverLoadConfig;

pub struct AgentDriverLoaderPlugin {
    root: PathBuf,
    schedule: String,
    limits: AgentDriverLoadLimits,
}
impl AgentDriverLoaderPlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, DriverLoadError> {
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
    pub fn with_limits(mut self, limits: AgentDriverLoadLimits) -> Self {
        self.limits = limits;
        self
    }
}
impl Plugin for AgentDriverLoaderPlugin {
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
            !app.world().contains_resource::<AgentDriverLoadConfig>(),
            "AgentDriverPlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "driver schedule does not exist"
        );
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut()
            .register_resources::<ResourceSet<AgentDriver>>("driver")
            .expect("driver resource registration failed");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(AgentDriverLoadConfig {
            root: self.root,
            schedule: self.schedule,
            limits: self.limits,
        });
        app.add_system(&schedule, system::load_agent_driver_system);
    }
}
#[cfg(test)]
mod tests;
