use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::execution_components::{AgentExecutionScope, AgentMclDrainHandle};
use crate::lua_runtime_components::{DriverLuaVmInstance, DriverWakeSlot, LuaRuntimeError};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq)]
pub enum LuaValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Array(Vec<LuaValue>),
    Object(BTreeMap<String, LuaValue>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaProgram {
    pub source: Arc<str>,
    pub origin: Arc<PathBuf>,
    pub libraries: LuaStandardLibraries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaStandardLibraries {
    Minimal,
    Safe,
    Full,
}

#[derive(Clone, Debug)]
pub struct LuaEnvironmentContext {
    pub instance: DriverLuaVmInstance,
    pub values: BTreeMap<String, LuaValue>,
    pub wake: Arc<Mutex<DriverWakeSlot>>,
    pub execution: AgentExecutionScope,
    pub mcl: AgentMclDrainHandle,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LuaEnvironment {
    pub globals: Vec<LuaGlobalBinding>,
    pub modules: Vec<LuaModuleBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LuaGlobalBinding {
    pub name: String,
    pub binding: LuaBindingValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LuaModuleBinding {
    pub name: String,
    pub exports: BTreeMap<String, LuaBindingValue>,
}

#[derive(Clone)]
pub enum LuaBindingValue {
    Value(LuaValue),
    Function(Arc<dyn LuaHostFunction>),
}

impl fmt::Debug for LuaBindingValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => formatter.debug_tuple("Value").field(value).finish(),
            Self::Function(_) => formatter.write_str("Function(..)"),
        }
    }
}

impl PartialEq for LuaBindingValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Value(left), Self::Value(right)) => left == right,
            (Self::Function(left), Self::Function(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

pub type HostFuture =
    Pin<Box<dyn Future<Output = Result<LuaValue, LuaRuntimeError>> + Send + 'static>>;

pub trait LuaHostFunction: Send + Sync + 'static {
    fn call(
        &self,
        arguments: LuaValue,
        context: LuaEnvironmentContext,
        cancel: CancellationToken,
    ) -> HostFuture;
}
