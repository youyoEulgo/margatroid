mod handler;
#[cfg(test)]
#[path = "../tests/internal/lifecycle.rs"]
mod lifecycle_tests;
mod system;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::{WorldClosureExt, WorldClosureSender};
use core_plugin::{App, Plugin, Resource};
use tool_core::{TaskSyncBinding, ToolExecutionLimits};

pub use handler::runner_main;

#[derive(Clone, Debug)]
pub struct ToolPlugin {
    pub(crate) schedule: String,
    pub(crate) runner_path: PathBuf,
    pub(crate) limits: ToolExecutionLimits,
    pub(crate) sync: TaskSyncBinding,
}

impl ToolPlugin {
    pub fn new(sync: TaskSyncBinding) -> Self {
        let runner_path = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("tool_runner")))
            .unwrap_or_else(|| PathBuf::from("tool_runner"));
        Self {
            sync,
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
        if let Some(clients) = app.world().get_resource::<client_core::Clients>() {
            assert!(
                clients.same_instance(&self.sync.clients),
                "tool sync must use the shared Clients slot"
            );
        } else {
            app.world_mut().insert_resource(self.sync.clients.clone());
        }
        let sender = Arc::new(app.world().closure_sender(&self.schedule));
        app.world_mut().insert_resource(ToolRuntime(sender));
        self.limits.validate().expect("ToolPlugin limits invalid");
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(self);
        app.add_system(&schedule, system::init_tool_tasks_system);
        app.add_system(&schedule, system::tool_execute_system);
    }
}
