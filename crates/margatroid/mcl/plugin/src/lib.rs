mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MclImportConfig {
    pub prompt_root: PathBuf,
    pub tool_root: PathBuf,
    pub sandbox_root: PathBuf,
}

impl Default for MclImportConfig {
    fn default() -> Self {
        Self {
            prompt_root: "resources/prompts".into(),
            tool_root: "resources/tools".into(),
            sandbox_root: "resources/sandboxes".into(),
        }
    }
}

impl Resource for MclImportConfig {}

#[derive(Clone, Debug)]
pub struct MclPlugin {
    pub schedule: String,
    pub imports: MclImportConfig,
}

impl Default for MclPlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            imports: MclImportConfig::default(),
        }
    }
}

impl MclPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs after DTO abort control, which executes in PRE_UPDATE.
    /// Only the runtime's UPDATE and POST_UPDATE schedules preserve that ordering.
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_import_config(mut self, imports: MclImportConfig) -> Self {
        self.imports = imports;
        self
    }
}

pub(crate) struct InstalledMcl {
    pub schedule: String,
}
impl Resource for InstalledMcl {}

impl Plugin for MclPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "MclPlugin requires RuntimePlugin and AsyncRuntimePlugin"
        );
        assert!(
            !app.world().contains_resource::<InstalledMcl>(),
            "MclPlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "MclPlugin schedule does not exist"
        );
        assert!(
            matches!(
                self.schedule.as_str(),
                RuntimePlugin::UPDATE | RuntimePlugin::POST_UPDATE
            ),
            "MclPlugin requires UPDATE or POST_UPDATE so PRE_UPDATE abort control runs first"
        );
        let _ = app.world().closure_sender(&self.schedule);
        app.world_mut().insert_resource(self.imports);
        app.world_mut().insert_resource(InstalledMcl {
            schedule: self.schedule.clone(),
        });
        app.add_system(&self.schedule, system::mcl_command_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};

    #[test]
    fn mcl_installation_requires_a_known_schedule_after_abort_control() {
        for schedule in [
            RuntimePlugin::UPDATE,
            RuntimePlugin::POST_UPDATE,
            RuntimePlugin::PRE_UPDATE,
            "custom",
        ] {
            let mut app = App::new();
            app.add_plugin(RuntimePlugin::default())
                .add_plugin(AsyncRuntimePlugin)
                .add_plugin(ClosurePlugin);
            if schedule == "custom" {
                app.add_schedule(schedule.into());
            }
            app.add_closure_system(schedule);
            let installed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                app.add_plugin(MclPlugin::new().with_schedule(schedule));
            }));
            assert_eq!(
                installed.is_ok(),
                matches!(schedule, RuntimePlugin::UPDATE | RuntimePlugin::POST_UPDATE),
                "schedule: {schedule}"
            );
        }
    }
}
