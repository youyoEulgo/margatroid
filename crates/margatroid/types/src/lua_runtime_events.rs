use crate::lua_environment::{LuaEnvironment, LuaProgram};
use crate::lua_runtime_components::{DriverLuaVmInstance, LuaRuntimeError};
use crate::workspace_execution::WorkspaceExecutionEntry;
use async_runtime_plugin::AsyncTaskError;
use core_plugin::{Entity, Event};

#[derive(Clone, Debug)]
pub struct StartDriverLuaVm {
    pub id: String,
    pub owner: Entity,
    pub program: LuaProgram,
    pub environment: LuaEnvironment,
    pub execution_entry: WorkspaceExecutionEntry,
}
impl Event for StartDriverLuaVm {}

#[derive(Clone, Debug)]
pub struct StartDriverLuaVmResult {
    pub id: String,
    pub owner: Entity,
    pub result: Result<(), LuaRuntimeError>,
}
impl Event for StartDriverLuaVmResult {}

#[derive(Clone, Debug)]
pub struct StopDriverLuaVm {
    pub id: String,
    pub owner: Entity,
    pub instance: DriverLuaVmInstance,
}
impl Event for StopDriverLuaVm {}

#[derive(Clone, Debug)]
pub struct StopDriverLuaVmResult {
    pub id: String,
    pub owner: Entity,
    pub instance: DriverLuaVmInstance,
    pub result: Result<(), LuaRuntimeError>,
}
impl Event for StopDriverLuaVmResult {}

#[derive(Clone, Debug)]
pub struct DriverLuaVmException {
    pub exception_id: String,
    pub instance: DriverLuaVmInstance,
    pub error: LuaRuntimeError,
    pub execution_ended: bool,
    pub cleanup_completed: bool,
}
impl Event for DriverLuaVmException {}

#[derive(Clone, Debug)]
pub struct DriverLuaVmExceptionResult {
    pub exception_id: String,
    pub instance: DriverLuaVmInstance,
    pub error: Option<LuaRuntimeError>,
    pub result: Result<(), LuaRuntimeError>,
    pub execution_ended: bool,
    pub cleanup_completed: bool,
}
impl Event for DriverLuaVmExceptionResult {}

#[derive(Clone, Debug)]
pub struct DriverLuaVmStartOutput {
    pub request: StartDriverLuaVm,
    pub instance: DriverLuaVmInstance,
    pub result: Result<(), LuaRuntimeError>,
}
#[derive(Clone, Debug)]
pub struct DriverLuaVmStopOutput {
    pub request: StopDriverLuaVm,
    pub instance: DriverLuaVmInstance,
    pub result: Result<(), LuaRuntimeError>,
}
#[derive(Clone, Debug)]
pub struct DriverLuaVmExceptionOutput {
    pub event: DriverLuaVmException,
    pub result: Result<(), LuaRuntimeError>,
    pub execution_ended: bool,
    pub cleanup_completed: bool,
}

#[derive(Debug)]
pub struct DriverLuaVmStartTaskError {
    pub request: Option<StartDriverLuaVm>,
    pub instance: Option<DriverLuaVmInstance>,
    pub source: AsyncTaskError,
}
impl From<AsyncTaskError> for DriverLuaVmStartTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self {
            request: None,
            instance: None,
            source,
        }
    }
}
#[derive(Debug)]
pub struct DriverLuaVmStopTaskError {
    pub request: Option<StopDriverLuaVm>,
    pub source: AsyncTaskError,
}
impl From<AsyncTaskError> for DriverLuaVmStopTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self {
            request: None,
            source,
        }
    }
}
#[derive(Debug)]
pub struct DriverLuaVmExceptionTaskError {
    pub event: Option<DriverLuaVmException>,
    pub source: AsyncTaskError,
}
impl From<AsyncTaskError> for DriverLuaVmExceptionTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self {
            event: None,
            source,
        }
    }
}
