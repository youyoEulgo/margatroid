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
            self.schedule != RuntimePlugin::STARTUP,
            "MclPlugin requires a recurring schedule"
        );
        let _ = app.world().closure_sender(&self.schedule);
        app.world_mut().insert_resource(self.imports);
        app.world_mut().insert_resource(InstalledMcl {
            schedule: self.schedule.clone(),
        });
        app.add_system(&self.schedule, system::mcl_command_system);
    }
}
