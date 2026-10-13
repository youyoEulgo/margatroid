use crate::lua_environment::LuaEnvironment;
use core_plugin::Resource;
use std::sync::Arc;

#[derive(Clone)]
pub struct MclEnvironmentFactory(pub Arc<dyn Fn() -> LuaEnvironment + Send + Sync>);
impl Resource for MclEnvironmentFactory {}
impl MclEnvironmentFactory {
    pub fn build(&self) -> LuaEnvironment {
        (self.0)()
    }
}
