mod handler;
mod system;
mod types;

use std::path::PathBuf;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};
use tool_core::ToolExecutionLimits;

pub use handler::runner_main;

#[derive(Clone, Debug)]
pub struct ToolPlugin {
    pub(crate) schedule: String,
    pub(crate) runner_path: PathBuf,
    pub(crate) limits: ToolExecutionLimits,
}

impl ToolPlugin {
    pub fn new() -> Self {
        let runner_path = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("tool_runner")))
            .unwrap_or_else(|| PathBuf::from("tool_runner"));
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            runner_path,
            limits: ToolExecutionLimits::default(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_runner_path(mut self, runner_path: impl Into<PathBuf>) -> Self {
        self.runner_path = runner_path.into();
        self
    }

    pub fn with_limits(mut self, limits: ToolExecutionLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Resource for ToolPlugin {}

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin required by ToolPlugin"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin required by ToolPlugin"
        );
        assert!(
            !app.world().contains_resource::<Self>(),
            "ToolPlugin already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "ToolPlugin schedule missing"
        );
        assert!(
            self.schedule != RuntimePlugin::STARTUP,
            "ToolPlugin requires recurring schedule"
        );
        app.world().send_closure(&self.schedule, |_| {});
        self.limits.validate().expect("ToolPlugin limits invalid");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(self);
        app.add_system(&schedule, system::init_tool_tasks_system);
        app.add_system(&schedule, system::tool_execute_system);
    }
}
