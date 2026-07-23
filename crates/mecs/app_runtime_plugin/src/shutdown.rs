use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use core_plugin::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShutdownPhase {
    Begin,
    StopIngress,
    StopWorkers,
    FlushState,
    Finish,
}

impl ShutdownPhase {
    const ALL: [Self; 5] = [
        Self::Begin,
        Self::StopIngress,
        Self::StopWorkers,
        Self::FlushState,
        Self::Finish,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

type ShutdownSystem = Box<dyn FnMut(&mut World) + Send + 'static>;

#[derive(Clone)]
pub(crate) struct ShutdownSystems {
    systems: Arc<Mutex<[Vec<ShutdownSystem>; 5]>>,
}

impl ShutdownSystems {
    pub(crate) fn new() -> Self {
        Self {
            systems: Arc::new(Mutex::new(std::array::from_fn(|_| Vec::new()))),
        }
    }

    pub(crate) fn add(
        &self,
        phase: ShutdownPhase,
        system: impl FnMut(&mut World) + Send + 'static,
    ) {
        self.systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[phase.index()]
        .push(Box::new(system));
    }

    pub(crate) fn run(&self, world: &mut World) {
        let mut systems = self
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for phase in ShutdownPhase::ALL {
            for system in &mut systems[phase.index()] {
                if catch_unwind(AssertUnwindSafe(|| system(world))).is_err() {
                    tracing::error!(?phase, "shutdown system panicked");
                }
            }
        }
    }
}
