use crate::{DriverLuaVmInstance, DriverWakeSlot, LuaRuntimeError};
use core_plugin::Component;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub type DriverReadySender = Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>;

#[derive(Debug)]
pub struct DriverLuaVm {
    pub instance: DriverLuaVmInstance,
    pub wake: Arc<Mutex<DriverWakeSlot>>,
    pub ready: DriverReadySender,
    pub ended: Option<oneshot::Receiver<Result<(), LuaRuntimeError>>>,
    pub cancel: CancellationToken,
}

impl Component for DriverLuaVm {}
