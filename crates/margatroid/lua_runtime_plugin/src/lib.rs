mod error;
mod events;
mod handler;
mod system;
mod types;

use std::sync::{Arc, RwLock};

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimeHandle;
use core_plugin::{App, Plugin, Resource};

pub use error::LuaRuntimeError;
pub use events::*;
pub use margatroid_types::LuaVmId;
pub use types::{
    CancellationToken, HostFuture, LuaBindingValue, LuaEnvironment, LuaEnvironmentContext,
    LuaEnvironmentProvider, LuaEnvironmentRegistry, LuaGlobalBinding, LuaHostFunction,
    LuaModuleBinding, LuaProgram, LuaRuntimeConfig, LuaRuntimeHandle, LuaRuntimeReply,
    LuaRuntimeResult, LuaScheduler, LuaStandardLibraries, LuaValue, LuaVmOwner, LuaVmSession,
    LuaVmState,
};

use crate::system::{
    lua_runtime_cancel_system, lua_runtime_request_system, lua_runtime_result_system,
    lua_vm_message_system, lua_vm_receive_system,
};
use crate::types::LuaRuntimeState;

pub struct LuaRuntimePlugin {
    schedule: String,
    config: LuaRuntimeConfig,
}

impl LuaRuntimePlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
            config: LuaRuntimeConfig::default(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_config(mut self, config: LuaRuntimeConfig) -> Self {
        self.config = config;
        self
    }
}

impl Default for LuaRuntimePlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LuaRuntimePluginInstalled;

impl Resource for LuaRuntimePluginInstalled {}

impl Plugin for LuaRuntimePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            panic!("LuaRuntimePlugin requires RuntimePlugin and AsyncRuntimePlugin");
        }
        if app.world().contains_resource::<LuaRuntimePluginInstalled>() {
            panic!("LuaRuntimePlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("LuaRuntimePlugin schedule does not exist");
        }
        let events = app.world().event_sender();
        let environments = Arc::new(RwLock::new(LuaEnvironmentRegistry::default()));
        app.world_mut().insert_resource(LuaRuntimeHandle {
            events,
            environments,
        });
        app.world_mut().insert_resource(self.config);
        app.world_mut().insert_resource(LuaRuntimeState::default());
        app.world_mut().insert_resource(LuaRuntimePluginInstalled);
        app.add_system(&self.schedule, lua_runtime_request_system)
            .add_system(&self.schedule, lua_runtime_cancel_system)
            .add_system(&self.schedule, lua_vm_message_system)
            .add_system(&self.schedule, lua_vm_receive_system)
            .add_system(&self.schedule, lua_runtime_result_system);
    }
}
