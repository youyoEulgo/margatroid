use std::time::Duration;

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

impl AsyncTaskControl {
    pub fn cancel(&self, id: AsyncTaskId) -> bool {
        self.spawner.cancel(id)
    }
}
