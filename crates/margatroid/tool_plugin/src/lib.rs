mod boundary;
mod handler;
mod process;
#[cfg(target_os = "linux")]
mod pty;
mod runner;
mod supervisor;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin, Resource};
pub use margatroid_types::tool_events::{
    ToolError, ToolErrorKind, ToolExecuteReply, ToolExecuteRequest, ToolExecuteResponse,
};
#[doc(hidden)]
pub use runner::runner_main;
use std::path::PathBuf;
pub use types::{RunnerRequest, RunnerResponse, ToolContext, ToolExecutionLimits};

#[derive(Clone, Debug)]
pub struct ToolPlugin {
    pub schedule: String,
    pub runner_path: PathBuf,
    pub limits: ToolExecutionLimits,
}
impl Default for ToolPlugin {
    fn default() -> Self {
        let runner_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("tool_runner")))
            .unwrap_or_else(|| PathBuf::from("tool_runner"));
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            runner_path,
            limits: ToolExecutionLimits::default(),
        }
    }
}
impl ToolPlugin {
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_runner_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.runner_path = path.into();
        self
    }
    pub fn with_limits(mut self, limits: ToolExecutionLimits) -> Self {
        self.limits = limits;
        self
    }
}
impl Resource for ToolPlugin {}
impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>(),
            "RuntimePlugin required"
        );
        assert!(
            app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AsyncRuntimePlugin required"
        );
        assert!(
            !app.world().contains_resource::<Self>(),
            "ToolPlugin already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "ToolPlugin schedule missing"
        );
        self.limits.validate().expect("ToolPlugin limits invalid");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(self);
        app.add_system(&schedule, system::tool_execute_system);
    }
}
