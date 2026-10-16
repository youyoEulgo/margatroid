use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use resource_id_plugin::ResourceID;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub type ActivityID = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionActivityKind {
    Inference,
    Sandbox,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionAdmission {
    Open,
    SandboxPaused,
    CleanupFailed,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    Closed,
    SandboxPaused,
    ExecutionBusy,
    AlreadyStarted,
    ActivityNotRegistered,
    ScopeDropped,
    ActivityIdExhausted,
    CleanupFailed {
        activity_id: ActivityID,
        message: String,
    },
    SnapshotFailed(String),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ExecutionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicySnapshot {
    pub resource_id: ResourceID,
    pub policy: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct ExecutionActivityState {
    pub kind: ExecutionActivityKind,
    pub cancel: CancellationToken,
    pub turn_cancel: CancellationToken,
    pub cleanup_error: Option<ExecutionError>,
    turn: Option<String>,
    started: bool,
}

#[derive(Debug)]
pub struct AgentExecutionData {
    admission: ExecutionAdmission,
    root_cancel: CancellationToken,
    activities: BTreeMap<ActivityID, ExecutionActivityState>,
    next_id: ActivityID,
    completion: Arc<Notify>,
}

#[derive(Clone, Debug)]
pub struct AgentExecutionScope {
    inner: Arc<Mutex<AgentExecutionData>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for AgentExecutionScope {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentExecutionScope {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(AgentExecutionData {
                admission: ExecutionAdmission::Open,
                root_cancel: CancellationToken::new(),
                activities: BTreeMap::new(),
                next_id: 1,
                completion: Arc::new(Notify::new()),
            })),
        }
    }

    pub fn child_cancel(&self) -> CancellationToken {
        lock(&self.inner).root_cancel.child_token()
    }

    pub fn admission(&self) -> ExecutionAdmission {
        lock(&self.inner).admission
    }

    pub fn is_closed(&self) -> bool {
        self.admission() == ExecutionAdmission::Closed
    }

    pub fn activity_count(&self) -> usize {
        lock(&self.inner).activities.len()
    }

    pub fn register(
        &self,
        kind: ExecutionActivityKind,
    ) -> Result<ExecutionActivity, ExecutionError> {
        self.register_with_snapshot(kind, || Ok(None))
    }

    /// The callback runs under the admission lock and must not await or reenter this scope.
    /// Inference and Other registrations never invoke the sandbox snapshot callback.
    pub fn register_with_snapshot<F>(
        &self,
        kind: ExecutionActivityKind,
        snapshot: F,
    ) -> Result<ExecutionActivity, ExecutionError>
    where
        F: FnOnce() -> Result<Option<SandboxPolicySnapshot>, ExecutionError>,
    {
        self.register_for_turn_with_snapshot(kind, None, snapshot)
    }

    pub fn register_for_turn(
        &self,
        kind: ExecutionActivityKind,
        turn: Option<String>,
    ) -> Result<ExecutionActivity, ExecutionError> {
        self.register_for_turn_with_snapshot(kind, turn, || Ok(None))
    }

    pub fn register_for_turn_with_snapshot<F>(
        &self,
        kind: ExecutionActivityKind,
        turn: Option<String>,
        snapshot: F,
    ) -> Result<ExecutionActivity, ExecutionError>
    where
        F: FnOnce() -> Result<Option<SandboxPolicySnapshot>, ExecutionError>,
    {
        let mut data = lock(&self.inner);
        check_admission(&data, kind)?;
        let next_id = data
            .next_id
            .checked_add(1)
            .ok_or(ExecutionError::ActivityIdExhausted)?;
        let sandbox_snapshot = if kind == ExecutionActivityKind::Sandbox {
            snapshot()?
        } else {
            None
        };
        let id = data.next_id;
        data.next_id = next_id;
        let cancel = data.root_cancel.child_token();
        let turn_cancel = CancellationToken::new();
        data.activities.insert(
            id,
            ExecutionActivityState {
                kind,
                cancel: cancel.clone(),
                turn_cancel: turn_cancel.clone(),
                turn,
                cleanup_error: None,
                started: false,
            },
        );
        Ok(ExecutionActivity {
            id,
            kind,
            cancel,
            turn_cancel,
            sandbox_snapshot,
            scope: Arc::downgrade(&self.inner),
        })
    }

    /// Validation and the actual configuration mutation must both happen in this callback.
    /// The callback must not await or reenter this scope. Equal IDs do not bypass this gate.
    pub fn with_sandbox_switch<T, F>(&self, switch: F) -> Result<T, ExecutionError>
    where
        F: FnOnce() -> Result<T, ExecutionError>,
    {
        self.with_sandbox_configuration(true, switch)
    }

    pub fn with_sandbox_configuration<T, F>(
        &self,
        changed: bool,
        update: F,
    ) -> Result<T, ExecutionError>
    where
        F: FnOnce() -> Result<T, ExecutionError>,
    {
        let data = lock(&self.inner);
        check_admission(&data, ExecutionActivityKind::Sandbox)?;
        if changed
            && data
                .activities
                .values()
                .any(|activity| activity.kind == ExecutionActivityKind::Sandbox)
        {
            return Err(ExecutionError::ExecutionBusy);
        }
        update()
    }

    pub fn cancel_turn(&self, turn: &str) {
        let data = lock(&self.inner);
        for activity in data.activities.values() {
            if activity.turn.as_deref() == Some(turn) {
                activity.turn_cancel.cancel();
            }
        }
        data.completion.notify_waiters();
    }

    pub fn turn_is_drained(&self, turn: &str) -> bool {
        !lock(&self.inner)
            .activities
            .values()
            .any(|activity| activity.turn.as_deref() == Some(turn))
    }

    pub fn request_close(&self) {
        let mut data = lock(&self.inner);
        data.admission = ExecutionAdmission::Closed;
        data.root_cancel.cancel();
        data.completion.notify_waiters();
    }

    /// Synchronously closes only the sandbox gate and requests child cancellation.
    /// Calling this after global close does not reopen the scope or cancel a new root.
    pub fn begin_sandbox_cleanup(&self) {
        let mut data = lock(&self.inner);
        if data.admission == ExecutionAdmission::Open {
            data.admission = ExecutionAdmission::SandboxPaused;
        }
        for activity in data.activities.values() {
            if activity.kind == ExecutionActivityKind::Sandbox {
                activity.cancel.cancel();
            }
        }
        data.completion.notify_waiters();
    }

    /// Restores sandbox admission only after every sandbox registration is released.
    /// Closed is terminal, including when a previously requested cleanup succeeds.
    pub fn resume_sandbox(&self) -> Result<(), ExecutionError> {
        let mut data = lock(&self.inner);
        if let Some(error) = activity_error(&data, Some(ExecutionActivityKind::Sandbox)) {
            return Err(error);
        }
        if data
            .activities
            .values()
            .any(|activity| activity.kind == ExecutionActivityKind::Sandbox)
        {
            return Err(ExecutionError::ExecutionBusy);
        }
        if data.admission != ExecutionAdmission::Closed {
            data.admission = ExecutionAdmission::Open;
        }
        data.completion.notify_waiters();
        Ok(())
    }

    pub async fn cleanup_sandbox(&self) -> Result<(), ExecutionError> {
        self.begin_sandbox_cleanup();
        self.wait_sandbox().await?;
        self.resume_sandbox()
    }

    pub async fn wait_sandbox(&self) -> Result<(), ExecutionError> {
        self.wait_for(Some(ExecutionActivityKind::Sandbox)).await
    }

    pub async fn wait_all(&self) -> Result<(), ExecutionError> {
        self.wait_for(None).await
    }

    async fn wait_for(&self, kind: Option<ExecutionActivityKind>) -> Result<(), ExecutionError> {
        let completion = lock(&self.inner).completion.clone();
        loop {
            let notified = completion.notified();
            tokio::pin!(notified);
            {
                let data = lock(&self.inner);
                notified.as_mut().enable();
                if let Some(error) = activity_error(&data, kind) {
                    return Err(error);
                }
                if !data
                    .activities
                    .values()
                    .any(|activity| kind.is_none_or(|kind| kind == activity.kind))
                {
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}

fn check_admission(
    data: &AgentExecutionData,
    kind: ExecutionActivityKind,
) -> Result<(), ExecutionError> {
    if data.admission == ExecutionAdmission::Closed {
        return Err(ExecutionError::Closed);
    }
    if kind == ExecutionActivityKind::Sandbox {
        match data.admission {
            ExecutionAdmission::SandboxPaused => return Err(ExecutionError::SandboxPaused),
            ExecutionAdmission::CleanupFailed => {
                return Err(
                    activity_error(data, Some(kind)).unwrap_or(ExecutionError::SandboxPaused)
                );
            }
            _ => {}
        }
    }
    Ok(())
}

fn activity_error(
    data: &AgentExecutionData,
    kind: Option<ExecutionActivityKind>,
) -> Option<ExecutionError> {
    data.activities
        .values()
        .filter(|activity| kind.is_none_or(|kind| kind == activity.kind))
        .find_map(|activity| activity.cleanup_error.clone())
}

/// Dropping a handle does not release its registration or prove execution cleanup.
/// The worker must mark started before execution; only failed, unstarted submissions may roll back.
#[derive(Debug)]
pub struct ExecutionActivity {
    pub id: ActivityID,
    pub kind: ExecutionActivityKind,
    pub cancel: CancellationToken,
    pub turn_cancel: CancellationToken,
    pub sandbox_snapshot: Option<SandboxPolicySnapshot>,
    scope: Weak<Mutex<AgentExecutionData>>,
}

impl ExecutionActivity {
    pub fn mark_started(&self) -> Result<(), ExecutionError> {
        let scope = self.scope.upgrade().ok_or(ExecutionError::ScopeDropped)?;
        let mut data = lock(&scope);
        let activity = data
            .activities
            .get_mut(&self.id)
            .ok_or(ExecutionError::ActivityNotRegistered)?;
        activity.started = true;
        Ok(())
    }

    /// The caller must prove exit, pipe closure and resource release before calling this.
    pub fn mark_completed(&self) -> Result<(), ExecutionError> {
        self.remove(false)
    }

    pub fn rollback_submission(&self) -> Result<(), ExecutionError> {
        self.remove(true)
    }

    fn remove(&self, rollback: bool) -> Result<(), ExecutionError> {
        let scope = self.scope.upgrade().ok_or(ExecutionError::ScopeDropped)?;
        let mut data = lock(&scope);
        if rollback
            && data
                .activities
                .get(&self.id)
                .is_some_and(|activity| activity.started || activity.cleanup_error.is_some())
        {
            return Err(ExecutionError::AlreadyStarted);
        }
        data.activities.remove(&self.id);
        data.completion.notify_waiters();
        Ok(())
    }

    pub fn mark_cleanup_failed(&self, error: ExecutionError) {
        if let Some(scope) = self.scope.upgrade() {
            let mut data = lock(&scope);
            if let Some(activity) = data.activities.get_mut(&self.id) {
                activity.cleanup_error = Some(error);
                if activity.kind == ExecutionActivityKind::Sandbox
                    && data.admission != ExecutionAdmission::Closed
                {
                    data.admission = ExecutionAdmission::CleanupFailed;
                }
                data.completion.notify_waiters();
            }
        }
    }
}

#[derive(Debug)]
struct MclDrainData {
    cleanup_error: Option<ExecutionError>,
    closed: bool,
    busy: bool,
    pending_cancel: Option<CancellationToken>,
    completion: Arc<Notify>,
}

/// Shared admission and receipt-drain proof only; the owner retains the actual FIFO.
#[derive(Clone, Debug)]
pub struct AgentMclDrainHandle {
    inner: Arc<Mutex<MclDrainData>>,
}

impl Default for AgentMclDrainHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentMclDrainHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MclDrainData {
                cleanup_error: None,
                closed: false,
                busy: false,
                pending_cancel: None,
                completion: Arc::new(Notify::new()),
            })),
        }
    }

    pub fn is_closed(&self) -> bool {
        lock(&self.inner).closed
    }

    pub fn is_drained(&self) -> bool {
        let data = lock(&self.inner);
        data.cleanup_error.is_none() && !data.busy && data.pending_cancel.is_none()
    }

    /// Call before admitting a new command; no new queue entry is allowed on failure.
    pub fn mark_busy(&self) -> Result<(), ExecutionError> {
        let mut data = lock(&self.inner);
        if data.closed {
            return Err(ExecutionError::Closed);
        }
        data.busy = true;
        Ok(())
    }

    /// Associates the current pending command, including an already-admitted command after close.
    /// A pending command installed after close is immediately cancelled, not silently discarded.
    pub fn set_pending_cancel(&self, cancel: Option<CancellationToken>) {
        let mut data = lock(&self.inner);
        if let Some(cancel) = &cancel {
            data.busy = true;
            if data.closed {
                cancel.cancel();
            }
        }
        data.pending_cancel = cancel;
    }

    pub fn close(&self) {
        let mut data = lock(&self.inner);
        data.closed = true;
        if let Some(cancel) = &data.pending_cancel {
            cancel.cancel();
        }
        data.completion.notify_waiters();
    }

    /// Only the queue owner may assert that commands, pending and all receipts are complete.
    pub fn mark_drained(&self) {
        let mut data = lock(&self.inner);
        data.busy = false;
        data.pending_cancel = None;
        data.completion.notify_waiters();
    }

    pub fn mark_cleanup_failed(&self, error: ExecutionError) {
        let mut data = lock(&self.inner);
        data.closed = true;
        data.cleanup_error = Some(error);
        data.completion.notify_waiters();
    }

    pub async fn wait_drained(&self) -> Result<(), ExecutionError> {
        let completion = lock(&self.inner).completion.clone();
        loop {
            let notified = completion.notified();
            tokio::pin!(notified);
            {
                let data = lock(&self.inner);
                notified.as_mut().enable();
                if let Some(error) = &data.cleanup_error {
                    return Err(error.clone());
                }
                if !data.busy && data.pending_cancel.is_none() {
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::time::Duration;

    #[test]
    fn turn_abort_only_signals_matching_turn_without_closing_root() {
        let scope = AgentExecutionScope::new();
        let current = scope
            .register_for_turn(ExecutionActivityKind::Inference, Some("current".into()))
            .unwrap();
        let other = scope
            .register_for_turn(ExecutionActivityKind::Sandbox, Some("other".into()))
            .unwrap();
        let root = scope.child_cancel();
        scope.cancel_turn("current");
        scope.cancel_turn("current");
        assert!(current.turn_cancel.is_cancelled());
        assert!(!current.cancel.is_cancelled());
        assert!(!other.turn_cancel.is_cancelled());
        assert!(!root.is_cancelled());
        assert_eq!(scope.admission(), ExecutionAdmission::Open);
        assert!(!scope.turn_is_drained("current"));
        current.mark_completed().unwrap();
        assert!(scope.turn_is_drained("current"));
        other.mark_completed().unwrap();
    }

    #[test]
    fn concurrent_registration_and_sandbox_cleanup_are_serialized() {
        for _ in 0..100 {
            let scope = AgentExecutionScope::new();
            let barrier = Arc::new(Barrier::new(2));
            let worker = {
                let scope = scope.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    scope.register(ExecutionActivityKind::Sandbox)
                })
            };
            barrier.wait();
            scope.begin_sandbox_cleanup();
            match worker.join().unwrap() {
                Ok(activity) => {
                    assert!(activity.cancel.is_cancelled());
                    activity.mark_completed().unwrap();
                }
                Err(error) => assert_eq!(error, ExecutionError::SandboxPaused),
            }
            assert_eq!(scope.activity_count(), 0);
        }
    }

    #[tokio::test]
    async fn cleanup_failure_retains_registration_and_returns_error() {
        let scope = AgentExecutionScope::new();
        let activity = scope.register(ExecutionActivityKind::Sandbox).unwrap();
        let inference = scope.register(ExecutionActivityKind::Inference).unwrap();
        scope.begin_sandbox_cleanup();
        assert!(!inference.cancel.is_cancelled());
        let error = ExecutionError::CleanupFailed {
            activity_id: activity.id,
            message: "pipe still open".into(),
        };
        activity.mark_cleanup_failed(error.clone());
        assert_eq!(scope.wait_all().await, Err(error.clone()));
        assert_eq!(scope.wait_sandbox().await, Err(error));
        assert_eq!(scope.activity_count(), 2);
        assert_eq!(scope.admission(), ExecutionAdmission::CleanupFailed);
        assert!(scope.register(ExecutionActivityKind::Other).is_ok());
        assert!(scope.register(ExecutionActivityKind::Sandbox).is_err());
        activity.mark_completed().unwrap();
        scope.resume_sandbox().unwrap();
        assert_eq!(scope.admission(), ExecutionAdmission::Open);
    }

    #[tokio::test]
    async fn successful_cleanup_cannot_reopen_closed_scope() {
        let scope = AgentExecutionScope::new();
        let sandbox = scope.register(ExecutionActivityKind::Sandbox).unwrap();
        let inference = scope.register(ExecutionActivityKind::Inference).unwrap();
        scope.begin_sandbox_cleanup();
        scope.request_close();
        assert!(inference.cancel.is_cancelled());
        sandbox.mark_completed().unwrap();
        scope.cleanup_sandbox().await.unwrap();
        assert_eq!(scope.admission(), ExecutionAdmission::Closed);
        assert!(matches!(
            scope.register(ExecutionActivityKind::Other),
            Err(ExecutionError::Closed)
        ));
        inference.mark_completed().unwrap();
        scope.wait_all().await.unwrap();
    }

    #[test]
    fn rollback_requires_unstarted_submission_and_drop_is_not_completion() {
        let scope = AgentExecutionScope::new();
        let unstarted = scope.register(ExecutionActivityKind::Other).unwrap();
        unstarted.rollback_submission().unwrap();
        unstarted.rollback_submission().unwrap();
        assert_eq!(
            unstarted.mark_started(),
            Err(ExecutionError::ActivityNotRegistered)
        );
        let started = scope.register(ExecutionActivityKind::Other).unwrap();
        started.mark_started().unwrap();
        assert_eq!(
            started.rollback_submission(),
            Err(ExecutionError::AlreadyStarted)
        );
        drop(started);
        assert_eq!(scope.activity_count(), 1);
    }

    #[test]
    fn sandbox_snapshot_failure_and_switch_are_atomic() {
        let scope = AgentExecutionScope::new();
        let error = ExecutionError::SnapshotFailed("not loaded".into());
        assert!(scope
            .register_with_snapshot(ExecutionActivityKind::Sandbox, || Err(error))
            .is_err());
        assert_eq!(scope.activity_count(), 0);
        let inference = scope
            .register_with_snapshot(ExecutionActivityKind::Inference, || panic!("not a sandbox"))
            .unwrap();
        scope.with_sandbox_switch(|| Ok(())).unwrap();
        let sandbox = scope.register(ExecutionActivityKind::Sandbox).unwrap();
        assert_eq!(
            scope.with_sandbox_switch(|| Ok(())),
            Err(ExecutionError::ExecutionBusy)
        );
        sandbox.mark_completed().unwrap();
        inference.mark_completed().unwrap();
        scope.with_sandbox_switch(|| Ok(())).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn completion_notifications_do_not_lose_wakeups() {
        for _ in 0..100 {
            let scope = AgentExecutionScope::new();
            let activity = scope.register(ExecutionActivityKind::Other).unwrap();
            let first = tokio::spawn({
                let scope = scope.clone();
                async move { scope.wait_all().await }
            });
            let second = tokio::spawn({
                let scope = scope.clone();
                async move { scope.wait_all().await }
            });
            tokio::task::yield_now().await;
            activity.mark_completed().unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                first.await.unwrap().unwrap();
                second.await.unwrap().unwrap();
                scope.wait_all().await.unwrap();
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_failure_wakes_existing_waiters_without_releasing_activity() {
        for _ in 0..100 {
            let scope = AgentExecutionScope::new();
            let activity = scope.register(ExecutionActivityKind::Sandbox).unwrap();
            let waiting = tokio::spawn({
                let scope = scope.clone();
                async move { scope.wait_all().await }
            });
            tokio::task::yield_now().await;
            let error = ExecutionError::CleanupFailed {
                activity_id: activity.id,
                message: "still running".into(),
            };
            activity.mark_cleanup_failed(error.clone());
            let result = tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result, Err(error));
            assert_eq!(scope.activity_count(), 1);
            assert_eq!(
                activity.rollback_submission(),
                Err(ExecutionError::AlreadyStarted)
            );
            activity.mark_completed().unwrap();
            activity.mark_completed().unwrap();
            scope.wait_all().await.unwrap();
        }
    }

    #[test]
    fn scope_snapshot_callback_excludes_concurrent_switch() {
        let scope = AgentExecutionScope::new();
        let barrier = Arc::new(Barrier::new(2));
        let worker = {
            let scope = scope.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                scope.register_with_snapshot(ExecutionActivityKind::Sandbox, || {
                    barrier.wait();
                    Ok(None)
                })
            })
        };
        barrier.wait();
        assert_eq!(
            scope.with_sandbox_switch(|| Ok(())),
            Err(ExecutionError::ExecutionBusy)
        );
        worker.join().unwrap().unwrap().mark_completed().unwrap();
        scope.with_sandbox_switch(|| Ok(())).unwrap();
    }

    #[tokio::test]
    async fn mcl_cleanup_failure_wakes_waiters_without_success_proof() {
        let drain = AgentMclDrainHandle::new();
        drain.mark_busy().unwrap();
        let error = ExecutionError::CleanupFailed {
            activity_id: 0,
            message: "transaction still open".into(),
        };
        let waiting = tokio::spawn({
            let drain = drain.clone();
            async move { drain.wait_drained().await }
        });
        tokio::task::yield_now().await;
        drain.mark_cleanup_failed(error.clone());
        assert_eq!(waiting.await.unwrap(), Err(error.clone()));
        drain.mark_drained();
        assert!(!drain.is_drained());
        assert_eq!(drain.wait_drained().await, Err(error));
        assert_eq!(drain.mark_busy(), Err(ExecutionError::Closed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcl_close_cancels_pending_but_waits_for_receipts() {
        for _ in 0..100 {
            let drain = AgentMclDrainHandle::new();
            drain.mark_busy().unwrap();
            let cancel = CancellationToken::new();
            drain.set_pending_cancel(Some(cancel.clone()));
            let waiting = tokio::spawn({
                let drain = drain.clone();
                async move { drain.wait_drained().await }
            });
            drain.close();
            assert!(cancel.is_cancelled());
            assert!(!drain.is_drained());
            assert_eq!(drain.mark_busy(), Err(ExecutionError::Closed));
            let late_cancel = CancellationToken::new();
            drain.set_pending_cancel(Some(late_cancel.clone()));
            assert!(late_cancel.is_cancelled());
            tokio::task::yield_now().await;
            drain.mark_drained();
            tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(drain.is_closed());
        }
    }
}
