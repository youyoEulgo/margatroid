mod catalog;
mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
pub use catalog::{scan_validated, MAX_CATALOG_DIRECTORY_ENTRIES};
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
pub use margatroid_types::agent_driver_events::{
    DriverLoadError, LoadAgentDriver, LoadAgentDriverResult,
};
pub use margatroid_types::resource_components::{AgentDriver, AgentDriverModelConfig};
pub use margatroid_types::workspace_catalog::DriverCatalog;
use resource_id_plugin::{ResourceID, ResourceSet, WorldResourceExt};
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
    /// Blocking validation for callers outside the ECS schedule.
    ///
    /// Runs in a fresh thread so callers already inside Tokio never nest runtimes.
    /// Missing or unreadable storage is an error; invalid packages are omitted.
    pub fn scan_catalog(&self) -> Result<Vec<ResourceID>, DriverLoadError> {
        let root = self.root.clone();
        let limits = self.limits;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| DriverLoadError::new(format!("cannot start catalog runtime: {e}")))?;
            runtime
                .block_on(catalog::scan_validated(&root, &limits))
                .map(|snapshot| snapshot.drivers)
        })
        .join()
        .map_err(|_| DriverLoadError::new("driver catalog worker panicked"))?
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
        if !app.world().contains_resource::<DriverCatalog>() {
            app.world_mut().insert_resource(DriverCatalog::default());
        }
        if !app
            .world()
            .contains_resource::<margatroid_types::discovery::AgentDriverDiscoverySnapshot>()
        {
            app.world_mut().insert_resource(
                margatroid_types::discovery::AgentDriverDiscoverySnapshot::default(),
            );
        }
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(AgentDriverLoadConfig {
            root: self.root,
            schedule: self.schedule,
            limits: self.limits,
        });
        app.add_system(&schedule, system::load_agent_driver_system);
        app.add_system(RuntimePlugin::PRE_UPDATE, system::driver_catalog_system);
        system::start_catalog_scan(app.world_mut(), "driver-startup".into());
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod catalog_tests;
