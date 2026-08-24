use std::time::Instant;

use core_plugin::Event;
use margatroid_types::LuaVmId;

use crate::error::LuaRuntimeError;
use crate::types::{
    LuaEnvironmentContext, LuaProgram, LuaRuntimeReply, LuaScheduler, LuaValue, LuaVmOwner,
    LuaVmState,
};

#[derive(Clone, Debug)]
pub struct LuaRuntimeRequest {
    pub request_id: String,
    pub owner: LuaVmOwner,
    pub program: LuaProgram,
    pub context: LuaEnvironmentContext,
    pub providers: Vec<String>,
    pub scheduler: LuaScheduler,
    pub deadline: Option<Instant>,
    pub reply: LuaRuntimeReply,
}
impl Event for LuaRuntimeRequest {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeCancelRequest {
    pub request_id: String,
    pub vm_id: Option<LuaVmId>,
}
impl Event for LuaRuntimeCancelRequest {}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaVmMessage {
    pub vm_id: LuaVmId,
    pub value: LuaValue,
}
impl Event for LuaVmMessage {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaVmMessageReceiveRequest {
    pub id: String,
    pub vm_id: LuaVmId,
}
impl Event for LuaVmMessageReceiveRequest {}
#[derive(Clone, Debug, PartialEq)]
pub struct LuaVmMessageReceived {
    pub id: String,
    pub vm_id: LuaVmId,
    pub result: Result<LuaValue, LuaRuntimeError>,
}
impl Event for LuaVmMessageReceived {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaVmStarted {
    pub request_id: String,
    pub vm_id: LuaVmId,
    pub owner: LuaVmOwner,
}
impl Event for LuaVmStarted {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeTaskFinished {
    pub request_id: String,
    pub vm_id: Option<LuaVmId>,
    pub owner: LuaVmOwner,
    pub state: LuaVmState,
    pub error: Option<LuaRuntimeError>,
}
impl Event for LuaRuntimeTaskFinished {}
