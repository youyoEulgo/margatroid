use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

struct AppControlInner {
    shutdown: AtomicBool,
    pending_wake: Mutex<bool>,
    wake: Condvar,
}

/// 可跨线程唤醒或停止 App 主循环的控制句柄。
#[derive(Clone)]
pub struct AppControl {
    inner: Arc<AppControlInner>,
}

impl AppControl {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(AppControlInner {
                shutdown: AtomicBool::new(false),
                pending_wake: Mutex::new(false),
                wake: Condvar::new(),
            }),
        }
    }

    pub fn wake(&self) {
        let mut pending = self
            .inner
            .pending_wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pending = true;
        self.inner.wake.notify_one();
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.wake();
    }

    pub fn is_shutdown(&self) -> bool {
        self.inner.shutdown.load(Ordering::Acquire)
    }

    pub(crate) fn wait(&self) {
        let mut pending = self
            .inner
            .pending_wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*pending && !self.is_shutdown() {
            pending = self
                .inner
                .wake
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *pending = false;
    }
}
