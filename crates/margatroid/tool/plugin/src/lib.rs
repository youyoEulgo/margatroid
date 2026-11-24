mod handler;
#[cfg(test)]
#[path = "../tests/internal/lifecycle.rs"]
mod lifecycle_tests;
mod services;
mod system;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{App, Plugin, Resource};
use tool_core::{ToolExecutionLimits, ToolTasks};
pub type TaskObserverBinder = Arc<dyn Fn(core_plugin::Entity, String, &ToolTasks) + Send + Sync>;

pub use handler::runner_main;

#[derive(Clone)]
pub struct ToolPlugin {
    pub(crate) schedule: String,
    pub(crate) runner_path: PathBuf,
    pub(crate) limits: ToolExecutionLimits,
    pub(crate) observe_tasks: Option<TaskObserverBinder>,
}

impl ToolPlugin {
    pub fn new() -> Self {
        let runner_path = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("tool_runner")))
            .unwrap_or_else(|| PathBuf::from("tool_runner"));
        Self {
            observe_tasks: None,
            schedule: RuntimePlugin::UPDATE.into(),
            runner_path,
            limits: ToolExecutionLimits::default(),
        }
    }

    pub fn with_task_observer(mut self, observer: TaskObserverBinder) -> Self {
        self.observe_tasks = Some(observer);
        self
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

impl Resource for ToolPlugin {}

struct ToolRuntime(Arc<WorldClosureSender>);
impl Resource for ToolRuntime {}

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
            matches!(
                self.schedule.as_str(),
                RuntimePlugin::UPDATE | RuntimePlugin::POST_UPDATE
            ),
            "ToolPlugin requires UPDATE or POST_UPDATE after turn abort handling"
        );
        let sender = Arc::new(app.world().closure_sender(&self.schedule));
        app.world_mut().insert_resource(ToolRuntime(sender));
        services::install_on(app, &self.schedule);
        self.limits.validate().expect("ToolPlugin limits invalid");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(self);
        app.add_system(&schedule, system::init_tool_tasks_system);
        app.add_system(&schedule, system::tool_execute_system);
    }
}

pub use services::install as install_lua_services;

impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../tests/support/lifecycle.rs"]
mod test_support;
