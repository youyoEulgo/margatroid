pub mod components;
mod error;
pub mod events;
pub mod host;
pub mod types;

pub use components::{DriverLuaVm, DriverReadySender};
pub use error::{LuaRuntimeError, LuaRuntimeErrorKind};
pub use events::{
    DriverLuaVmExited, StartDriverLuaVm, StartDriverLuaVmReply, StopDriverLuaVm,
    StopDriverLuaVmReply,
};
pub use types::{
    DriverLuaVmInstance, DriverWakeSlot, HostFuture, LuaBinding, LuaEnvironment, LuaHostFunction,
    LuaProgram, LuaRuntimeConfig, LuaStandardLibraries, LuaValue,
};

use core_plugin::World;
use types::lock;

pub trait WorldLuaRuntimeExt {
    fn driver_ready(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError>;
    fn driver_wake(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError>;
}

impl WorldLuaRuntimeExt for World {
    fn driver_ready(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError> {
        let vm = checked_vm(self, instance)?;
        let sender = lock(&vm.ready).take().ok_or_else(|| {
            LuaRuntimeError::new(
                LuaRuntimeErrorKind::AlreadyReady,
                "Driver ready was already consumed",
            )
        })?;
        sender.send(Ok(())).map_err(|_| {
            LuaRuntimeError::new(
                LuaRuntimeErrorKind::Closed,
                "Driver startup waiter is closed",
            )
        })
    }

    fn driver_wake(&self, instance: &DriverLuaVmInstance) -> Result<(), LuaRuntimeError> {
        let vm = checked_vm(self, instance)?;
        lock(&vm.wake).wake()
    }
}

fn checked_vm<'a>(
    world: &'a World,
    instance: &DriverLuaVmInstance,
) -> Result<&'a DriverLuaVm, LuaRuntimeError> {
    let vm = world
        .get_component::<DriverLuaVm>(instance.owner)
        .ok_or_else(|| {
            LuaRuntimeError::new(
                LuaRuntimeErrorKind::InstanceNotFound,
                "Driver VM is missing",
            )
        })?;
    if vm.instance != *instance {
        return Err(LuaRuntimeError::new(
            LuaRuntimeErrorKind::InstanceMismatch,
            "Driver VM instance does not match",
        ));
    }
    Ok(vm)
}
