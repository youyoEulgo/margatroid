use crate::{DriverLuaVmInstance, LuaEnvironment, LuaProgram, LuaRuntimeError};
use core_plugin::{Entity, Event};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type StartDriverLuaVmReply =
    Arc<Mutex<Option<oneshot::Sender<Result<DriverLuaVmInstance, LuaRuntimeError>>>>>;
pub type StopDriverLuaVmReply = Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>;

#[derive(Clone, Debug)]
pub struct StartDriverLuaVm {
    pub owner: Entity,
    pub workspace: Entity,
    pub program: LuaProgram,
    pub environment: LuaEnvironment,
    pub reply: StartDriverLuaVmReply,
}

impl Event for StartDriverLuaVm {}

#[derive(Clone, Debug)]
pub struct StopDriverLuaVm {
    pub instance: DriverLuaVmInstance,
    pub reply: StopDriverLuaVmReply,
}

impl Event for StopDriverLuaVm {}

#[derive(Clone, Debug)]
pub struct DriverLuaVmExited {
    pub instance: DriverLuaVmInstance,
    pub result: Result<(), LuaRuntimeError>,
}

impl Event for DriverLuaVmExited {}
