mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};
use std::path::PathBuf;

pub struct AgentPlugin {
    pub schedule: String,
    driver_root: PathBuf,
    sync: agent_core::AgentUpdateContext,
}
impl AgentPlugin {
    pub fn new(sync: agent_core::AgentUpdateContext) -> Self {
        Self {
            sync,
            schedule: RuntimePlugin::UPDATE.into(),
            driver_root: PathBuf::from("resources/agent-drivers"),
        }
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn with_resource_roots(mut self, data_root: impl Into<PathBuf>) -> Self {
        self.driver_root = data_root.into().join("agent-drivers");
        self
    }
}
#[derive(Clone)]
pub(crate) struct AgentInstalled {
    pub schedule: String,
    pub driver_root: PathBuf,
    pub sync: agent_core::AgentUpdateContext,
}
impl Resource for AgentInstalled {}
impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<AgentInstalled>(),
            "AgentPlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule)
                && app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "AgentPlugin requires schedule, RuntimePlugin and AsyncRuntimePlugin"
        );
        let _ = app.world().closure_sender(&self.schedule);
        app.world_mut().insert_resource(AgentInstalled {
            sync: self.sync,
            schedule: self.schedule.clone(),
            driver_root: self.driver_root,
        });
        app.add_system(&self.schedule, system::stop_agent_system)
            .add_system(&self.schedule, system::restart_agent_system)
            .add_system(&self.schedule, system::start_agent_system)
            .add_system(&self.schedule, system::agent_exception_system);
    }
}
