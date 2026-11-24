mod handler;
mod system;
mod types;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin};
use std::path::PathBuf;
use workspace_core::WorkspaceConfig;

pub struct WorkspacePlugin {
    pub root: PathBuf,
}
impl Plugin for WorkspacePlugin {
    fn build(self, app: &mut App) {
        assert!(self.root.is_absolute(), "Workspace root must be absolute");
        assert!(
            !app.world().contains_resource::<WorkspaceConfig>(),
            "WorkspacePlugin already installed"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin required"
        );
        app.world_mut()
            .insert_resource(WorkspaceConfig { root: self.root });
        app.world_mut()
            .insert_resource(workspace_core::WorkspacePendingWork::default());
        app.add_system(RuntimePlugin::STARTUP, system::load_workspace_system)
            .add_system(RuntimePlugin::UPDATE, system::create_workspace_system)
            .add_system(RuntimePlugin::UPDATE, system::start_workspace_system)
            .add_system(RuntimePlugin::UPDATE, system::delete_workspace_system)
            .add_system(RuntimePlugin::UPDATE, system::start_agent_system)
            .add_system(RuntimePlugin::UPDATE, system::archive_agent_system);
    }
}
