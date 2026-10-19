mod catalog;
#[cfg(test)]
mod catalog_tests;
mod discovery;
mod handler;
mod system;
#[cfg(test)]
mod tests;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin};
use resource_id_plugin::WorldResourceExt;

pub use margatroid_types::workspace_components::*;
pub use margatroid_types::workspace_events::*;

pub struct WorkspacePlugin {
    schedule: String,
}
impl WorkspacePlugin {
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
impl Default for WorkspacePlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
}
impl Plugin for WorkspacePlugin {
    fn build(self, app: &mut App) {
        if !app.contains_schedule(&self.schedule)
            || !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            panic!("WorkspacePlugin requires schedule, RuntimePlugin and AsyncRuntimePlugin");
        }
        app.world_mut()
            .register_library::<Workspace>("workspace")
            .expect("WorkspacePlugin requires unified resource registry");
        app.world_mut()
            .insert_resource(catalog::Operations::default());
        if !app
            .world()
            .contains_resource::<margatroid_types::workspace_catalog::WorkspaceCatalog>()
        {
            app.world_mut()
                .insert_resource(margatroid_types::workspace_catalog::WorkspaceCatalog::default());
        }
        app.add_system(&self.schedule, discovery::system)
            .add_system(&self.schedule, catalog::system)
            .add_system(&self.schedule, system::start_workspace_system)
            .add_system(&self.schedule, system::stop_workspace_system)
            .add_system(&self.schedule, system::workspace_exception_system);
    }
}
