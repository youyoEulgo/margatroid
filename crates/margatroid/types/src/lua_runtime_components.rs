use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use core_plugin::Entity;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverLuaVmInstance {
    pub owner: Entity,
    pub start_request_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaRuntimeErrorKind {
    InvalidRequest,
    InstanceNotFound,
    InstanceMismatch,
    AlreadyReady,
    NotReady,
    SourceTooLarge,
    EnvironmentFailed,
    CompileFailed,
    ExecutionFailed,
    StartupTimeout,
    Cancelled,
    CleanupFailed,
    TaskFailed,
    StopAlreadyWaiting,
    EndNotificationLost,
    AlreadyWaiting,
    Closed,
    WakeNotificationLost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaRuntimeError {
    pub kind: LuaRuntimeErrorKind,
    pub description: String,
}

impl LuaRuntimeError {
    pub fn new(kind: LuaRuntimeErrorKind, description: impl Into<String>) -> Self {
        let mut description = description.into();
        if description.len() > 512 {
            let mut end = 509;
            while !description.is_char_boundary(end) {
                end -= 1;
            }
            description.truncate(end);
            description.push_str("...");
        }
        Self { kind, description }
    }
}

impl fmt::Display for LuaRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}:{}", self.kind, self.description)
    }
}

impl std::error::Error for LuaRuntimeError {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn error(kind: LuaRuntimeErrorKind) -> LuaRuntimeError {
    LuaRuntimeError::new(kind, format!("{kind:?}"))
}

#[derive(Debug, Default)]
pub struct DriverWakeSlot {
    pub sender: Option<oneshot::Sender<()>>,
    pub pending_wake: bool,
    pub closed: bool,
    pub waiting: bool,
}

impl DriverWakeSlot {
    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Only one wait may be in flight, including after wake has taken its sender.
    /// Cancellation or dropping this future releases waiting and preserves an unconsumed wake.
    pub async fn wait(
        slot: &Arc<Mutex<Self>>,
        cancellation: &CancellationToken,
    ) -> Result<(), LuaRuntimeError> {
        let receiver = {
            let mut data = lock(slot);
            if data.closed {
                return Err(error(LuaRuntimeErrorKind::Closed));
            }
            if data.waiting {
                return Err(error(LuaRuntimeErrorKind::AlreadyWaiting));
            }
            if cancellation.is_cancelled() {
                return Err(error(LuaRuntimeErrorKind::Cancelled));
            }
            if data.pending_wake {
                data.pending_wake = false;
                return Ok(());
            }
            let (sender, receiver) = oneshot::channel();
            data.sender = Some(sender);
            data.waiting = true;
            receiver
        };
        let mut guard = WakeWaitGuard {
            slot: slot.clone(),
            receiver,
            consumed: false,
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(error(LuaRuntimeErrorKind::Cancelled)),
            result = &mut guard.receiver => {
                guard.consumed = true;
                match result {
                    Ok(()) => Ok(()),
                    Err(_) => Err(error(if lock(slot).closed { LuaRuntimeErrorKind::Closed } else { LuaRuntimeErrorKind::WakeNotificationLost })),
                }
            }
        };
        if lock(slot).closed {
            return Err(error(LuaRuntimeErrorKind::Closed));
        }
        result
    }

    /// Multiple wakes without a waiter coalesce into one pending notification.
    pub fn wake(slot: &Arc<Mutex<Self>>) -> Result<(), LuaRuntimeError> {
        let mut data = lock(slot);
        if data.closed {
            return Err(error(LuaRuntimeErrorKind::Closed));
        }
        if let Some(sender) = data.sender.take() {
            if sender.send(()).is_err() {
                data.pending_wake = true;
            }
        } else {
            data.pending_wake = true;
        }
        Ok(())
    }

    pub fn close(slot: &Arc<Mutex<Self>>) {
        let mut data = lock(slot);
        data.closed = true;
        data.pending_wake = false;
        data.sender.take();
    }
}

struct WakeWaitGuard {
    slot: Arc<Mutex<DriverWakeSlot>>,
    receiver: oneshot::Receiver<()>,
    consumed: bool,
}

impl Drop for WakeWaitGuard {
    fn drop(&mut self) {
        let mut data = lock(&self.slot);
        data.sender.take();
        if !self.consumed && !data.closed && self.receiver.try_recv().is_ok() {
            data.pending_wake = true;
        }
        data.waiting = false;
    }
}

#[derive(Clone, Debug)]
pub struct DriverReadySlot {
    pub instance: DriverLuaVmInstance,
    pub sender: Arc<Mutex<Option<oneshot::Sender<Result<(), LuaRuntimeError>>>>>,
}

impl DriverReadySlot {
    pub fn new(
        instance: DriverLuaVmInstance,
    ) -> (Self, oneshot::Receiver<Result<(), LuaRuntimeError>>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                instance,
                sender: Arc::new(Mutex::new(Some(sender))),
            },
            receiver,
        )
    }

    /// Returns true only for the path that took the shared sender, even if its receiver is gone.
    pub fn complete(&self, result: Result<(), LuaRuntimeError>) -> bool {
        if let Some(sender) = lock(&self.sender).take() {
            let _ = sender.send(result);
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct DriverEndedSlot {
    pub instance: DriverLuaVmInstance,
    pub receiver: oneshot::Receiver<Result<(), LuaRuntimeError>>,
}

#[derive(Debug)]
pub struct DriverCloseData {
    pub instance: DriverLuaVmInstance,
    pub cancellation: CancellationToken,
    pub stop_requested: bool,
}

#[derive(Clone, Debug)]
pub struct DriverCloseHandle {
    inner: Arc<Mutex<DriverCloseData>>,
}

impl DriverCloseHandle {
    pub fn new(instance: DriverLuaVmInstance) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DriverCloseData {
                instance,
                cancellation: CancellationToken::new(),
                stop_requested: false,
            })),
        }
    }

    pub fn instance(&self) -> DriverLuaVmInstance {
        lock(&self.inner).instance.clone()
    }

    pub fn cancellation(&self) -> CancellationToken {
        lock(&self.inner).cancellation.clone()
    }

    pub fn is_close_requested(&self) -> bool {
        lock(&self.inner).stop_requested
    }

    /// Cancellation requests execution shutdown, not successful resource cleanup.
    pub fn request_close(&self) {
        let mut data = lock(&self.inner);
        data.stop_requested = true;
        data.cancellation.cancel();
    }
}

#[derive(Debug)]
pub struct AgentLua {
    pub instance: Option<DriverLuaVmInstance>,
    pub wake: Arc<Mutex<DriverWakeSlot>>,
    pub ready: Option<DriverReadySlot>,
    pub ended: Option<DriverEndedSlot>,
    pub close: Option<DriverCloseHandle>,
    pub handled_exceptions: std::collections::HashSet<String>,
}

impl Default for AgentLua {
    fn default() -> Self {
        Self {
            instance: None,
            wake: DriverWakeSlot::shared(),
            ready: None,
            ended: None,
            close: None,
            handled_exceptions: std::collections::HashSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_plugin::World;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    fn instance() -> DriverLuaVmInstance {
        DriverLuaVmInstance {
            owner: World::default().spawn(),
            start_request_id: "start-1".into(),
        }
    }

    fn poll_pending<F: Future>(future: std::pin::Pin<&mut F>) {
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.poll(&mut context), Poll::Pending));
    }

    #[tokio::test]
    async fn wake_before_wait_is_latched_once() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        DriverWakeSlot::wake(&slot).unwrap();
        DriverWakeSlot::wake(&slot).unwrap();
        DriverWakeSlot::wait(&slot, &cancel).await.unwrap();
        assert!(!lock(&slot).pending_wake);
        let future = DriverWakeSlot::wait(&slot, &cancel);
        tokio::pin!(future);
        poll_pending(future.as_mut());
        DriverWakeSlot::wake(&slot).unwrap();
        future.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_releases_wait_and_preserves_delivered_wake() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        let future = DriverWakeSlot::wait(&slot, &cancel);
        tokio::pin!(future);
        poll_pending(future.as_mut());
        DriverWakeSlot::wake(&slot).unwrap();
        cancel.cancel();
        assert_eq!(
            future.await.unwrap_err().kind,
            LuaRuntimeErrorKind::Cancelled
        );
        assert!(!lock(&slot).waiting);
        assert!(lock(&slot).pending_wake);
        DriverWakeSlot::wait(&slot, &CancellationToken::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_wait_releases_slot_without_losing_wake() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        let mut future = Box::pin(DriverWakeSlot::wait(&slot, &cancel));
        poll_pending(future.as_mut());
        DriverWakeSlot::wake(&slot).unwrap();
        drop(future);
        assert!(!lock(&slot).waiting);
        DriverWakeSlot::wait(&slot, &cancel).await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_wait_without_wake_allows_another_wait() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        let future = DriverWakeSlot::wait(&slot, &cancel);
        tokio::pin!(future);
        poll_pending(future.as_mut());
        cancel.cancel();
        assert_eq!(
            future.await.unwrap_err().kind,
            LuaRuntimeErrorKind::Cancelled
        );
        assert!(!lock(&slot).waiting);
        let next_cancel = CancellationToken::new();
        let next = DriverWakeSlot::wait(&slot, &next_cancel);
        tokio::pin!(next);
        poll_pending(next.as_mut());
        DriverWakeSlot::wake(&slot).unwrap();
        next.await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_wait_is_rejected_even_after_sender_is_taken() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        let first = DriverWakeSlot::wait(&slot, &cancel);
        tokio::pin!(first);
        poll_pending(first.as_mut());
        DriverWakeSlot::wake(&slot).unwrap();
        assert_eq!(
            DriverWakeSlot::wait(&slot, &cancel).await.unwrap_err().kind,
            LuaRuntimeErrorKind::AlreadyWaiting
        );
        first.await.unwrap();
        assert!(!lock(&slot).waiting);
    }

    #[tokio::test]
    async fn close_wakes_receiver_with_error_and_rejects_new_waits() {
        let slot = DriverWakeSlot::shared();
        let cancel = CancellationToken::new();
        let future = DriverWakeSlot::wait(&slot, &cancel);
        tokio::pin!(future);
        poll_pending(future.as_mut());
        DriverWakeSlot::close(&slot);
        assert_eq!(future.await.unwrap_err().kind, LuaRuntimeErrorKind::Closed);
        assert_eq!(
            DriverWakeSlot::wait(&slot, &cancel).await.unwrap_err().kind,
            LuaRuntimeErrorKind::Closed
        );
        assert!(DriverWakeSlot::wake(&slot).is_err());
        assert!(!lock(&slot).waiting);
    }

    #[tokio::test]
    async fn ready_sender_is_shared_and_taken_only_once() {
        let (ready, receiver) = DriverReadySlot::new(instance());
        let other = ready.clone();
        assert!(ready.complete(Ok(())));
        assert!(!other.complete(Err(error(LuaRuntimeErrorKind::TaskFailed))));
        receiver.await.unwrap().unwrap();
        assert!(lock(&other.sender).is_none());
    }

    #[tokio::test]
    async fn concurrent_wake_and_wait_drop_do_not_lose_notification() {
        for _ in 0..100 {
            let slot = DriverWakeSlot::shared();
            let cancel = CancellationToken::new();
            let mut future = Box::pin(DriverWakeSlot::wait(&slot, &cancel));
            poll_pending(future.as_mut());
            let thread = {
                let slot = slot.clone();
                std::thread::spawn(move || DriverWakeSlot::wake(&slot).unwrap())
            };
            drop(future);
            thread.join().unwrap();
            assert!(!lock(&slot).waiting);
            assert!(lock(&slot).pending_wake);
            DriverWakeSlot::wait(&slot, &cancel).await.unwrap();
        }
    }

    #[test]
    fn runtime_error_description_is_bounded_on_utf8_boundary() {
        let error = LuaRuntimeError::new(LuaRuntimeErrorKind::ExecutionFailed, "错误".repeat(200));
        assert!(error.description.len() <= 512);
        assert!(error.description.ends_with("..."));
    }

    #[test]
    fn close_is_shared_idempotent_cancellation_only() {
        let instance = instance();
        let close = DriverCloseHandle::new(instance.clone());
        let cloned = close.clone();
        assert!(!close.cancellation().is_cancelled());
        close.request_close();
        cloned.request_close();
        assert!(cloned.cancellation().is_cancelled());
        assert!(close.is_close_requested());
        assert_eq!(cloned.instance(), instance);
        let lua = AgentLua::default();
        assert!(
            lua.instance.is_none()
                && lua.ready.is_none()
                && lua.ended.is_none()
                && lua.close.is_none()
        );
    }
}
