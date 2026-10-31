mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin, Resource};

pub struct StoragePlugin {
    pub schedule: String,
}
impl Default for StoragePlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
}
impl StoragePlugin {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
pub(crate) struct StorageInstalled {
    pub(crate) schedule: String,
}
impl Resource for StorageInstalled {}
impl Plugin for StoragePlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<StorageInstalled>(),
            "StoragePlugin is already installed"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "StoragePlugin requires AsyncRuntimePlugin"
        );
        app.world_mut().insert_resource(StorageInstalled {
            schedule: self.schedule.clone(),
        });
        app.add_system(&self.schedule, system::storage_system);
    }
}
