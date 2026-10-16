mod handler;
mod system;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use core_plugin::{App, Plugin, Resource};
use resource_id_plugin::{ResourcePluginInstalled, WorldResourceExt};

pub use handler::*;
pub use margatroid_types::agent_components::{Agent, AgentInfo, AgentState, WorldAgentExt};
pub use margatroid_types::agent_events::*;
pub use system::*;

pub struct AgentPlugin {
    schedule: String,
}
impl AgentPlugin {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}
impl Default for AgentPlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
}
pub struct AgentPluginInstalled;
impl Resource for AgentPluginInstalled {}
impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<AgentPluginInstalled>(),
            "AgentPlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule)
                && app.world().contains_resource::<RuntimeHandle>(),
            "AgentPlugin requires RuntimePlugin"
        );
        assert!(
            app.world()
                .contains_resource::<async_runtime_plugin::AsyncRuntimeHandle>(),
            "AgentPlugin requires AsyncRuntimePlugin"
        );
        assert!(
            app.world().contains_resource::<ResourcePluginInstalled>(),
            "AgentPlugin requires ResourcePlugin"
        );
        app.world_mut()
            .register_library::<Agent>("agent")
            .expect("agent library registration failed");
        app.world_mut().insert_resource(AgentPluginInstalled);
        app.add_system(RuntimePlugin::PRE_UPDATE, abort_agent_turn_system);
        app.add_system(&self.schedule, start_agent_system)
            .add_system(&self.schedule, stop_agent_system)
            .add_system(&self.schedule, agent_exception_system);
    }
}
