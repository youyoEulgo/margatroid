use core_plugin::Resource;
use margatroid_types::lua_runtime_components::{LuaRuntimeError, LuaRuntimeErrorKind};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct LuaRuntimeConfig {
    pub max_source_bytes: usize,
    pub max_memory_bytes: usize,
    pub instruction_check_interval: u32,
    pub startup_timeout: Option<Duration>,
}
impl Resource for LuaRuntimeConfig {}
impl Default for LuaRuntimeConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            instruction_check_interval: 1000,
            startup_timeout: None,
        }
    }
}
impl LuaRuntimeConfig {
    pub fn validate(&self) -> Result<(), LuaRuntimeError> {
        if self.max_source_bytes == 0
            || self.max_memory_bytes == 0
            || self.instruction_check_interval == 0
            || self
                .startup_timeout
                .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::InvalidRequest,
                "Lua limits must be positive",
            ));
        }
        Ok(())
    }
}

pub(crate) struct LuaRuntimeSchedule(pub String);
impl Resource for LuaRuntimeSchedule {}
