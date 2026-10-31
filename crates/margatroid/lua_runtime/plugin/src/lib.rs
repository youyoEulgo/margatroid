mod handler;
mod system;
mod types;

pub use lua_runtime_core::{LuaRuntimeConfig, LuaRuntimeError, LuaRuntimeErrorKind};

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin};
use types::InstalledRuntime;

#[derive(Clone, Debug)]
pub struct LuaRuntimePlugin {
    pub schedule: String,
    pub config: LuaRuntimeConfig,
}

impl Default for LuaRuntimePlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            config: LuaRuntimeConfig::default(),
        }
    }
}

impl LuaRuntimePlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_config(mut self, config: LuaRuntimeConfig) -> Result<Self, LuaRuntimeError> {
        config.validate()?;
        self.config = config;
        Ok(self)
    }
}

impl Plugin for LuaRuntimePlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "LuaRuntimePlugin requires RuntimePlugin and AsyncRuntimePlugin"
        );
        assert!(
            !app.world().contains_resource::<InstalledRuntime>(),
            "LuaRuntimePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule),
            "LuaRuntimePlugin schedule does not exist"
        );
        self.config.validate().expect("invalid Lua runtime budgets");
        app.world().send_closure(&self.schedule, |_| {});
        let schedule = self.schedule.clone();
        app.world_mut().insert_resource(InstalledRuntime {
            schedule: self.schedule,
            config: self.config,
        });
        app.add_system(&schedule, system::stop_driver_lua_vm_system)
            .add_system(&schedule, system::start_driver_lua_vm_system)
            .add_system(&schedule, system::driver_lua_vm_exit_system);
    }
}
