use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use core_plugin::World;

type ShutdownSystem = Box<dyn FnMut(&mut World) + Send + 'static>;

#[derive(Clone)]
pub(crate) struct ShutdownSystems {
    systems: Arc<Mutex<Vec<ShutdownSystem>>>,
    finalizers: Arc<Mutex<Vec<ShutdownSystem>>>,
}

impl ShutdownSystems {
    pub(crate) fn new() -> Self {
        Self {
            systems: Arc::new(Mutex::new(Vec::new())),
            finalizers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn add(&self, system: impl FnMut(&mut World) + Send + 'static) {
        self.systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Box::new(system));
    }

    pub(crate) fn add_finalizer(&self, system: impl FnMut(&mut World) + Send + 'static) {
        self.finalizers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Box::new(system));
    }

    pub(crate) fn run(&self, world: &mut World) {
        let mut systems = self
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for system in systems.iter_mut().rev() {
            if catch_unwind(AssertUnwindSafe(|| system(world))).is_err() {
                tracing::error!("shutdown system panicked");
            }
        }
        let mut finalizers = self
            .finalizers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for finalizer in finalizers.iter_mut() {
            if catch_unwind(AssertUnwindSafe(|| finalizer(world))).is_err() {
                tracing::error!("shutdown finalizer panicked");
            }
        }
    }
}
