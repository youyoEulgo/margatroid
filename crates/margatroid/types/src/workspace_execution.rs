use std::any::Any;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc;
use tokio::task::{JoinSet, LocalSet};

pub type WorkspaceLocalTask = Pin<Box<dyn Future<Output = ()> + 'static>>;
pub type WorkspaceExecutionTask = Box<dyn FnOnce() -> WorkspaceLocalTask + Send + 'static>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    StartFailed(String),
    Closed,
    TaskPanicked(String),
    ThreadPanicked(String),
    CloseFailed(String),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartFailed(message) => {
                write!(formatter, "workspace execution start failed: {message}")
            }
            Self::Closed => formatter.write_str("workspace execution is closed"),
            Self::TaskPanicked(message) => write!(formatter, "workspace task panicked: {message}"),
            Self::ThreadPanicked(message) => {
                write!(formatter, "workspace thread panicked: {message}")
            }
            Self::CloseFailed(message) => {
                write!(formatter, "workspace execution close failed: {message}")
            }
        }
    }
}
impl std::error::Error for ExecutionError {}

pub enum WorkspaceExecutionCommand {
    Spawn(WorkspaceExecutionTask),
    Close,
}

struct Admission {
    closed: bool,
    close_sent: bool,
    sender: mpsc::UnboundedSender<WorkspaceExecutionCommand>,
}

/// Low-level submission capability; domain World APIs must enforce Running separately.
#[derive(Clone)]
pub struct WorkspaceExecutionEntry {
    admission: Arc<Mutex<Admission>>,
}

impl fmt::Debug for WorkspaceExecutionEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceExecutionEntry")
            .field("closed", &lock(&self.admission).closed)
            .finish()
    }
}

impl WorkspaceExecutionEntry {
    /// Admission and queue insertion share a lock with close, including all entry clones.
    pub fn submit(&self, task: WorkspaceExecutionTask) -> Result<(), ExecutionError> {
        let mut admission = lock(&self.admission);
        if admission.closed {
            return Err(ExecutionError::Closed);
        }
        if admission
            .sender
            .send(WorkspaceExecutionCommand::Spawn(task))
            .is_err()
        {
            admission.closed = true;
            return Err(ExecutionError::Closed);
        }
        Ok(())
    }

    /// Rejects new tasks without stopping the thread or cancelling existing tasks.
    /// Used when the domain enters Stopping or Failed, before Agent cleanup completes.
    pub fn close_admission(&self) {
        lock(&self.admission).closed = true;
    }

    pub fn is_closed(&self) -> bool {
        let admission = lock(&self.admission);
        admission.closed || admission.sender.is_closed()
    }
}

struct JoinState {
    handle: Option<JoinHandle<Result<(), ExecutionError>>>,
    result: Option<Result<(), ExecutionError>>,
}

struct SharedJoin {
    state: Mutex<JoinState>,
    completed: Condvar,
}

/// Closing drains tasks rather than cancelling Agent execution or replacing its cleanup.
#[derive(Clone)]
pub struct WorkspaceExecutionClose {
    entry: WorkspaceExecutionEntry,
    join: Arc<SharedJoin>,
}

impl fmt::Debug for WorkspaceExecutionClose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceExecutionClose")
            .field("entry", &self.entry)
            .field("finished", &lock(&self.join.state).result.is_some())
            .finish()
    }
}

impl WorkspaceExecutionClose {
    /// All concurrent and repeated callers observe the same cached thread-join result.
    /// Cancelling a caller does not discard the join handle or interrupt accepted tasks.
    pub async fn close(&self) -> Result<(), ExecutionError> {
        {
            let mut admission = lock(&self.entry.admission);
            admission.closed = true;
            if !admission.close_sent {
                admission.close_sent = true;
                let _ = admission.sender.send(WorkspaceExecutionCommand::Close);
            }
        }
        let join = self.join.clone();
        tokio::task::spawn_blocking(move || join.wait())
            .await
            .map_err(|error| ExecutionError::CloseFailed(error.to_string()))?
    }
}

impl SharedJoin {
    fn wait(&self) -> Result<(), ExecutionError> {
        let mut state = lock(&self.state);
        loop {
            if let Some(result) = &state.result {
                return result.clone();
            }
            if let Some(handle) = state.handle.take() {
                drop(state);
                let result = handle.join().unwrap_or_else(|panic| {
                    Err(ExecutionError::ThreadPanicked(panic_message(panic)))
                });
                state = lock(&self.state);
                state.result = Some(result.clone());
                self.completed.notify_all();
                return result;
            }
            state = self
                .completed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = panic.downcast_ref::<&str>() {
        return (*message).into();
    }
    "non-string panic payload".into()
}

/// Starts the executor eagerly and returns only after runtime initialization succeeds.
/// Factories cross the channel; their non-Send futures are constructed on this thread.
pub fn spawn_workspace_execution(
) -> Result<(WorkspaceExecutionEntry, WorkspaceExecutionClose), ExecutionError> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    let handle = thread::Builder::new()
        .name("workspace-execution".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let error = ExecutionError::StartFailed(error.to_string());
                    let _ = ready_sender.send(Err(error.clone()));
                    return Err(error);
                }
            };
            let local = LocalSet::new();
            if ready_sender.send(Ok(())).is_err() {
                return Ok(());
            }
            runtime.block_on(local.run_until(run(receiver)))
        })
        .map_err(|error| ExecutionError::StartFailed(error.to_string()))?;
    match ready_receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = handle.join();
            return Err(error);
        }
        Err(error) => {
            let _ = handle.join();
            return Err(ExecutionError::StartFailed(error.to_string()));
        }
    }
    let entry = WorkspaceExecutionEntry {
        admission: Arc::new(Mutex::new(Admission {
            closed: false,
            close_sent: false,
            sender,
        })),
    };
    let close = WorkspaceExecutionClose {
        entry: entry.clone(),
        join: Arc::new(SharedJoin {
            state: Mutex::new(JoinState {
                handle: Some(handle),
                result: None,
            }),
            completed: Condvar::new(),
        }),
    };
    Ok((entry, close))
}

async fn run(
    mut receiver: mpsc::UnboundedReceiver<WorkspaceExecutionCommand>,
) -> Result<(), ExecutionError> {
    let mut tasks = JoinSet::new();
    let mut failure = None;
    loop {
        tokio::select! {
            command = receiver.recv() => match command {
                Some(WorkspaceExecutionCommand::Spawn(factory)) => {
                    tasks.spawn_local(async move { factory().await; });
                }
                Some(WorkspaceExecutionCommand::Close) | None => {
                    receiver.close();
                    break;
                }
            },
            result = tasks.join_next(), if !tasks.is_empty() => {
                record_result(result, &mut failure);
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        record_result(Some(result), &mut failure);
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn record_result(
    result: Option<Result<(), tokio::task::JoinError>>,
    failure: &mut Option<ExecutionError>,
) {
    if let Some(Err(error)) = result {
        if failure.is_none() {
            *failure = Some(if error.is_panic() {
                ExecutionError::TaskPanicked(panic_message(error.into_panic()))
            } else {
                ExecutionError::CloseFailed(error.to_string())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn non_send_futures_and_multiple_tasks_finish_on_one_workspace_thread() {
        let (entry, close) = spawn_workspace_execution().unwrap();
        let completed = Arc::new(AtomicUsize::new(0));
        let caller = thread::current().id();
        let mut replies = Vec::new();
        for index in 0..8 {
            let completed = completed.clone();
            let (reply, receive) = oneshot::channel();
            entry
                .submit(Box::new(move || {
                    let owner = thread::current().id();
                    let local = Rc::new(index);
                    Box::pin(async move {
                        tokio::task::yield_now().await;
                        assert_eq!(*local, index);
                        assert_eq!(owner, thread::current().id());
                        completed.fetch_add(1, Ordering::SeqCst);
                        reply.send(owner).unwrap();
                    })
                }))
                .unwrap();
            replies.push(receive);
        }
        close.close().await.unwrap();
        let owner = replies.remove(0).await.unwrap();
        assert_ne!(owner, caller);
        for reply in replies {
            assert_eq!(reply.await.unwrap(), owner);
        }
        assert_eq!(completed.load(Ordering::SeqCst), 8);
    }

    #[tokio::test]
    async fn concurrent_close_waits_for_all_accepted_tasks_and_rejects_old_entry_clones() {
        let (entry, close) = spawn_workspace_execution().unwrap();
        let old_entry = entry.clone();
        let (release, wait) = oneshot::channel();
        let (started, receive_started) = oneshot::channel();
        let completed = Arc::new(AtomicUsize::new(0));
        let finished = completed.clone();
        entry
            .submit(Box::new(move || {
                Box::pin(async move {
                    started.send(()).unwrap();
                    wait.await.unwrap();
                    finished.store(1, Ordering::SeqCst);
                })
            }))
            .unwrap();
        receive_started.await.unwrap();
        let first_close = close.clone();
        let first = tokio::spawn(async move { first_close.close().await });
        let second_close = close.clone();
        let second = tokio::spawn(async move { second_close.close().await });
        while !entry.is_closed() {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            old_entry.submit(Box::new(|| Box::pin(async {}))),
            Err(ExecutionError::Closed)
        );
        assert!(!first.is_finished());
        assert!(!second.is_finished());
        release.send(()).unwrap();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(close.close().await, Ok(()));
    }

    #[tokio::test]
    async fn task_and_factory_panics_are_reaped_without_cancelling_other_tasks() {
        for factory_panic in [false, true] {
            let (entry, close) = spawn_workspace_execution().unwrap();
            entry
                .submit(Box::new(move || {
                    if factory_panic {
                        panic!("factory failed");
                    }
                    Box::pin(async {
                        panic!("task failed");
                    })
                }))
                .unwrap();
            let completed = Arc::new(AtomicUsize::new(0));
            let finished = completed.clone();
            entry
                .submit(Box::new(move || {
                    Box::pin(async move {
                        tokio::task::yield_now().await;
                        finished.store(1, Ordering::SeqCst);
                    })
                }))
                .unwrap();
            let error = close.close().await.unwrap_err();
            assert!(matches!(error, ExecutionError::TaskPanicked(_)));
            assert_eq!(completed.load(Ordering::SeqCst), 1);
            assert_eq!(close.close().await, Err(error));
        }
    }

    #[tokio::test]
    async fn admission_can_close_before_cleanup_without_terminating_execution() {
        let (entry, close) = spawn_workspace_execution().unwrap();
        let (release, wait) = oneshot::channel();
        let (completed, finished) = oneshot::channel();
        entry
            .submit(Box::new(move || {
                Box::pin(async move {
                    wait.await.unwrap();
                    completed.send(()).unwrap();
                })
            }))
            .unwrap();
        entry.close_admission();
        assert!(entry.is_closed());
        assert_eq!(
            entry.submit(Box::new(|| Box::pin(async {}))),
            Err(ExecutionError::Closed)
        );
        release.send(()).unwrap();
        finished.await.unwrap();
        close.close().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_close_caller_does_not_lose_the_thread_join() {
        let (entry, close) = spawn_workspace_execution().unwrap();
        let (release, wait) = oneshot::channel();
        entry
            .submit(Box::new(move || {
                Box::pin(async move {
                    wait.await.unwrap();
                })
            }))
            .unwrap();
        let abandoned_close = close.clone();
        let abandoned = tokio::spawn(async move { abandoned_close.close().await });
        while !entry.is_closed() {
            tokio::task::yield_now().await;
        }
        abandoned.abort();
        let _ = abandoned.await;
        release.send(()).unwrap();
        close.close().await.unwrap();
    }
}
