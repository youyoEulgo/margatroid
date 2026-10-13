mod handler;
mod system;
#[cfg(test)]
mod tests;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};

pub use margatroid_types::lua_environment::*;
pub use margatroid_types::lua_runtime_components::*;
pub use margatroid_types::lua_runtime_events::*;
pub use tokio_util::sync::CancellationToken;
pub use types::LuaRuntimeConfig;

use system::{
    driver_lua_vm_exception_system, start_driver_lua_vm_system, stop_driver_lua_vm_system,
};
use types::LuaRuntimeSchedule;

pub struct LuaRuntimePlugin {
    schedule: String,
    config: LuaRuntimeConfig,
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
impl Default for LuaRuntimePlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            config: LuaRuntimeConfig::default(),
        }
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
        self.config.validate().expect("invalid Lua limits");
        app.world().send_closure(&self.schedule, |_| {});
        app.world_mut().insert_resource(self.config);
        app.world_mut()
            .insert_resource(LuaRuntimeSchedule(self.schedule.clone()));
        app.world_mut().insert_resource(LuaRuntimePluginInstalled);
        app.add_system(&self.schedule, start_driver_lua_vm_system)
            .add_system(&self.schedule, stop_driver_lua_vm_system)
            .add_system(&self.schedule, driver_lua_vm_exception_system);
    }
}
