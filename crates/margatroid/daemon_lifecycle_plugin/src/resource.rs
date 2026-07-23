use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum DaemonState {
    #[default]
    Starting = 0,
    Ready = 1,
    Draining = 2,
    Stopped = 3,
}

#[derive(Clone, Default)]
pub struct DaemonLifecycle {
    state: Arc<AtomicU8>,
}

impl DaemonLifecycle {
    pub fn state(&self) -> DaemonState {
        match self.state.load(Ordering::Acquire) {
            0 => DaemonState::Starting,
            1 => DaemonState::Ready,
            2 => DaemonState::Draining,
            3 => DaemonState::Stopped,
            _ => unreachable!("invalid daemon lifecycle state"),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state() == DaemonState::Ready
    }

    pub(crate) fn set(&self, state: DaemonState) {
        self.state.store(state as u8, Ordering::Release);
    }
}
