mod error;
mod events;
mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use core_plugin::{App, Component, Plugin, Resource};
use lua_runtime_plugin::{
    LuaBindingValue, LuaEnvironment, LuaEnvironmentContext, LuaEnvironmentProvider,
    LuaGlobalBinding, LuaRuntimeError, LuaRuntimeHandle, LuaRuntimePluginInstalled,
};
use resource_id_plugin::ResourceIdPluginInstalled;

pub use error::{failure, AgentCreateResult, AgentFailureKind};
pub use events::*;
pub use handler::{
    control_stop, handle_agent_control, handle_agent_create, handle_agent_initialization_completed,
    handle_agent_message, handle_lua_vm_finished, handle_lua_vm_started,
};
pub use margatroid_types::{
    AgentError, AgentErrorKind, AgentLuaMessageEnvelope, AgentMessage, MclMessage,
};
pub use system::{
    agent_control_system, agent_create_system, agent_lua_vm_state_system, agent_message_system,
};
pub use types::*;

#[derive(Clone, Debug)]
pub struct Agent {
    pub info: AgentInfo,
    pub creation: AgentCreationState,
    pub mcl: AgentMcl,
    pub resources: AgentResourceMap,
    pub memory: AgentMemoryHandle,
    pub inference: AgentInferenceState,
    pub tools: AgentToolState,
    pub lua: AgentLuaState,
    pub lifecycle: AgentLifecycleState,
    pub turn: AgentTurnState,
    pub token_usage: TokenUsageState,
    pub last_error: Option<AgentError>,
}

impl Agent {
    pub fn info(&self) -> &AgentInfo {
        &self.info
    }

    pub fn mcl(&self) -> &AgentMcl {
        &self.mcl
    }

    pub fn mcl_mut(&mut self) -> &mut AgentMcl {
        &mut self.mcl
    }
}

impl Component for Agent {}

pub struct AgentPlugin {
    schedule: String,
}

impl AgentPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for AgentPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AgentPluginInstalled;
impl Resource for AgentPluginInstalled {}

impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<AgentPluginInstalled>() {
            panic!("AgentPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule)
            || !app.world().contains_resource::<RuntimeHandle>()
        {
            panic!("AgentPlugin requires RuntimePlugin");
        }
        if !app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("AgentPlugin requires ResourceIdPlugin");
        }
        if !app.world().contains_resource::<LuaRuntimePluginInstalled>() {
            panic!("AgentPlugin requires LuaRuntimePlugin");
        }

        app.world_mut().insert_resource(AgentPluginInstalled);
        app.world()
            .get_resource::<LuaRuntimeHandle>()
            .expect("LuaRuntimePlugin installed without LuaRuntimeHandle")
            .register_provider(Box::new(AgentInfoEnvironmentProvider))
            .expect("agent_info provider registration failed");

        app.add_system(&self.schedule, agent_create_system)
            .add_system(&self.schedule, agent_control_system)
            .add_system(&self.schedule, agent_message_system)
            .add_system(&self.schedule, agent_lua_vm_state_system);
    }
}

struct AgentInfoEnvironmentProvider;

impl LuaEnvironmentProvider for AgentInfoEnvironmentProvider {
    fn name(&self) -> &str {
        "agent_info"
    }

    fn provide(&self, context: &LuaEnvironmentContext) -> Result<LuaEnvironment, LuaRuntimeError> {
        let value = context.values.get("agent_info").cloned().ok_or_else(|| {
            LuaRuntimeError::EnvironmentFailed("agent_info is missing".to_owned())
        })?;
        Ok(LuaEnvironment {
            globals: vec![LuaGlobalBinding {
                name: "agent_info".to_owned(),
                binding: LuaBindingValue::Value(value),
            }],
            modules: Vec::new(),
        })
    }
}
