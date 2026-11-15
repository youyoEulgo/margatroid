use crate::LuaRuntimeError;
use core_plugin::{Entity, Resource};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Debug)]
pub struct LuaExecutionIdentity {
    pub source: Entity,
    pub workspace: Entity,
    pub address: String,
    pub instance: String,
    pub cancel: CancellationToken,
}
pub type LuaServiceFuture = Pin<Box<dyn Future<Output = Result<Value, LuaRuntimeError>> + Send>>;
type Invoke = Arc<dyn Fn(LuaExecutionIdentity, String, Value) -> LuaServiceFuture + Send + Sync>;
#[derive(Clone)]
pub struct LuaServices {
    invoke: Invoke,
}
impl Resource for LuaServices {}
impl LuaServices {
    pub fn new(
        invoke: impl Fn(LuaExecutionIdentity, String, Value) -> LuaServiceFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            invoke: Arc::new(invoke),
        }
    }
    pub fn call(
        &self,
        identity: LuaExecutionIdentity,
        method: String,
        arguments: Value,
    ) -> LuaServiceFuture {
        (self.invoke)(identity, method, arguments)
    }
}
