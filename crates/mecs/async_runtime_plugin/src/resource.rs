use std::time::Duration;

use crate::runtime::AsyncRuntimeState;
use crate::runtime::AsyncSpawner;
use crate::AsyncTaskId;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug)]
pub(crate) struct AsyncRuntimeOptions {
    pub(crate) queue_capacity: usize,
    pub(crate) max_in_flight: usize,
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
    pub(crate) timeout: Option<Duration>,
}

impl Default for AsyncSystemOptions {
    fn default() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }
}

impl AsyncSystemOptions {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }

    pub fn without_timeout() -> Self {
        Self { timeout: None }
    }
}

#[derive(Clone)]
pub struct AsyncTasks {
    pub(crate) spawner: AsyncSpawner,
}

#[derive(Clone)]
pub struct AsyncRuntimeStatus {
    pub(crate) state: std::sync::Arc<AsyncRuntimeState>,
}

impl AsyncRuntimeStatus {
    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub(crate) fn shutdown(&self) {
        self.state.shutdown();
    }
}

impl AsyncTasks {
    pub fn cancel(&self, id: AsyncTaskId) -> bool {
        self.spawner.cancel(id)
    }
}
