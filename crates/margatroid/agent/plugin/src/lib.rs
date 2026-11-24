mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};

pub struct AgentPlugin {
    pub schedule: String,
    sync: agent_core::AgentUpdateContext,
}
impl AgentPlugin {
    pub fn new(sync: agent_core::AgentUpdateContext) -> Self {
        Self {
            sync,
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
#[derive(Clone)]
pub(crate) struct AgentInstalled {
    pub schedule: String,
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
        app.world_mut().insert_resource(self.sync.clone());
        app.world_mut().insert_resource(AgentInstalled {
            sync: self.sync,
            schedule: self.schedule.clone(),
        });
        app.add_system(&self.schedule, system::stop_agent_system)
            .add_system(&self.schedule, system::start_agent_system);
    }
}
