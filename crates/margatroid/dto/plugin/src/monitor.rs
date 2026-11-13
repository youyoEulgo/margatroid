use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
pub(crate) struct MonitorCompletion {
    done: AtomicBool,
    changed: tokio::sync::Notify,
}
pub(crate) struct MonitorCompletionGuard(pub Arc<MonitorCompletion>);
impl Drop for MonitorCompletionGuard {
    fn drop(&mut self) {
        self.0.done.store(true, Ordering::Release);
        self.0.changed.notify_waiters();
    }
}
impl MonitorCompletion {
    pub fn wait(self: Arc<Self>) -> agent_core::AgentObservationWait {
        Box::pin(async move {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.done.load(Ordering::Acquire) {
                    return Ok(());
                }
                changed.await;
            }
        })
    }
}
