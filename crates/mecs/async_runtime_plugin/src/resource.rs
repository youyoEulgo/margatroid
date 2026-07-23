use std::time::Duration;

use crate::runtime::AsyncRuntimeState;
use crate::runtime::AsyncSpawner;
use crate::AsyncTaskId;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug)]
pub struct AsyncRuntimeOptions {
    pub queue_capacity: usize,
    pub max_in_flight: usize,
}

impl Default for AsyncRuntimeOptions {
    fn default() -> Self {
        Self {
            queue_capacity: 1024,
            max_in_flight: 256,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AsyncSystemOptions {
    pub timeout: Option<Duration>,
}

impl Default for AsyncSystemOptions {
    fn default() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }
}

#[derive(Clone)]
pub struct AsyncTaskControl {
    pub(crate) spawner: AsyncSpawner,
}

#[derive(Clone)]
pub struct AsyncRuntimeHandle {
    pub(crate) state: std::sync::Arc<AsyncRuntimeState>,
}

impl AsyncRuntimeHandle {
    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub fn shutdown(&self) {
        self.state.shutdown();
    }
}

impl AsyncTaskControl {
    pub fn cancel(&self, id: AsyncTaskId) -> bool {
        self.spawner.cancel(id)
    }
}
