use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use app_runtime_plugin::AppControl;
use core_plugin::{Event, World};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::task::{AbortHandle, Id as TokioTaskId, JoinSet};

use crate::{
    AsyncRuntimeOptions, AsyncSystemOptions, AsyncTaskControl, AsyncTaskFailed,
    AsyncTaskFailureKind, AsyncTaskId,
};

type WorldCommand = Box<dyn FnOnce(&mut World) + Send + 'static>;
type TaskResult = Result<WorldCommand, AsyncTaskFailed>;
type AsyncTaskFuture = Pin<Box<dyn Future<Output = TaskResult> + Send + 'static>>;

struct AsyncTask {
    id: AsyncTaskId,
    request_type: &'static str,
    future: AsyncTaskFuture,
}

enum ControlMessage {
    Cancel(AsyncTaskId),
    Shutdown,
}

pub(crate) enum Completion {
    Apply(WorldCommand),
    Failed(AsyncTaskFailed),
}

#[derive(Clone)]
pub(crate) struct AsyncSpawner {
    task_sender: tokio_mpsc::Sender<AsyncTask>,
    control_sender: tokio_mpsc::UnboundedSender<ControlMessage>,
    next_id: Arc<AtomicU64>,
}

impl AsyncSpawner {
    pub(crate) fn spawn<Fut, Output>(
        &self,
        future: Fut,
        request_type: &'static str,
        options: AsyncSystemOptions,
    ) -> Result<AsyncTaskId, AsyncTaskFailed>
    where
        Fut: Future<Output = Output> + Send + 'static,
        Output: Event,
    {
        let id = AsyncTaskId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let execute = async move {
            let output = future.await;
            Box::new(move |world: &mut World| world.send_event(output)) as WorldCommand
        };
        let task_future: AsyncTaskFuture = match options.timeout {
            Some(timeout) => Box::pin(async move {
                tokio::time::timeout(timeout, execute)
                    .await
                    .map_err(|_| AsyncTaskFailed {
                        task_id: id,
                        request_type,
                        kind: AsyncTaskFailureKind::Timeout,
                        message: format!("task timed out after {timeout:?}"),
                    })
            }),
            None => Box::pin(async move { Ok(execute.await) }),
        };
        self.task_sender
            .try_send(AsyncTask {
                id,
                request_type,
                future: task_future,
            })
            .map_err(|error| {
                let (kind, message) = match error {
                    tokio_mpsc::error::TrySendError::Full(_) => (
                        AsyncTaskFailureKind::QueueFull,
                        "async task queue is full".to_string(),
                    ),
                    tokio_mpsc::error::TrySendError::Closed(_) => (
                        AsyncTaskFailureKind::WorkerStopped,
                        "async worker has stopped".to_string(),
                    ),
                };
                AsyncTaskFailed {
                    task_id: id,
                    request_type,
                    kind,
                    message,
                }
            })?;
        Ok(id)
    }

    pub(crate) fn cancel(&self, id: AsyncTaskId) -> bool {
        self.control_sender.send(ControlMessage::Cancel(id)).is_ok()
    }
}

pub(crate) struct AsyncRuntimeState {
    options: AsyncRuntimeOptions,
    worker: Mutex<Option<AsyncWorker>>,
    completion_receiver: Mutex<Option<mpsc::Receiver<Completion>>>,
}

impl AsyncRuntimeState {
    pub(crate) fn new(options: AsyncRuntimeOptions) -> Self {
        Self {
            options,
            worker: Mutex::new(None),
            completion_receiver: Mutex::new(None),
        }
    }

    pub(crate) fn start(&self, control: Option<AppControl>) -> Option<AsyncTaskControl> {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.is_some() {
            return None;
        }
        let (new_worker, receiver) = AsyncWorker::start(control, self.options);
        let task_control = AsyncTaskControl {
            spawner: new_worker.spawner(),
        };
        *self
            .completion_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(receiver);
        *worker = Some(new_worker);
        Some(task_control)
    }

    pub(crate) fn spawner(&self) -> Option<AsyncSpawner> {
        self.worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(AsyncWorker::spawner)
    }

    pub(crate) fn drain_completions(&self) -> Vec<Completion> {
        self.completion_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(AsyncWorker::is_running)
    }

    pub(crate) fn shutdown(&self) {
        self.worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

struct AsyncWorker {
    spawner: AsyncSpawner,
    thread: Option<thread::JoinHandle<()>>,
}

impl AsyncWorker {
    fn start(
        control: Option<AppControl>,
        options: AsyncRuntimeOptions,
    ) -> (Self, mpsc::Receiver<Completion>) {
        assert!(
            options.queue_capacity > 0,
            "async queue capacity must be positive"
        );
        assert!(
            options.max_in_flight > 0,
            "max in-flight tasks must be positive"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build async worker runtime");
        let (task_sender, task_receiver) = tokio_mpsc::channel(options.queue_capacity);
        let (control_sender, control_receiver) = tokio_mpsc::unbounded_channel();
        let (completion_sender, completion_receiver) = mpsc::channel();
        let spawner = AsyncSpawner {
            task_sender,
            control_sender,
            next_id: Arc::new(AtomicU64::new(1)),
        };
        let thread = thread::Builder::new()
            .name("margatroid-async-runtime".into())
            .spawn(move || {
                runtime.block_on(worker_loop(
                    task_receiver,
                    control_receiver,
                    completion_sender,
                    control,
                    options.max_in_flight,
                ));
            })
            .expect("failed to start async worker thread");
        (
            Self {
                spawner,
                thread: Some(thread),
            },
            completion_receiver,
        )
    }

    fn spawner(&self) -> AsyncSpawner {
        self.spawner.clone()
    }

    fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for AsyncWorker {
    fn drop(&mut self) {
        let _ = self.spawner.control_sender.send(ControlMessage::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct ActiveTask {
    id: AsyncTaskId,
    request_type: &'static str,
    abort: AbortHandle,
}

async fn worker_loop(
    mut task_receiver: tokio_mpsc::Receiver<AsyncTask>,
    mut control_receiver: tokio_mpsc::UnboundedReceiver<ControlMessage>,
    completion_sender: mpsc::Sender<Completion>,
    control: Option<AppControl>,
    max_in_flight: usize,
) {
    let mut tasks = JoinSet::new();
    let mut active_by_id = HashMap::<AsyncTaskId, TokioTaskId>::new();
    let mut active_by_tokio = HashMap::<TokioTaskId, ActiveTask>::new();
    let mut pending_cancellations = HashSet::new();
    let mut highest_seen_id = 0;

    loop {
        tokio::select! {
            task = task_receiver.recv(), if active_by_id.len() < max_in_flight => {
                let Some(task) = task else {
                    abort_all(&mut tasks).await;
                    break;
                };
                highest_seen_id = highest_seen_id.max(task.id.get());
                if pending_cancellations.remove(&task.id) {
                    send_completion(
                        &completion_sender,
                        Completion::Failed(cancelled_failure(task.id, task.request_type)),
                        control.as_ref(),
                    );
                    continue;
                }
                let task_id = task.id;
                let request_type = task.request_type;
                let abort = tasks.spawn(async move { (task_id, task.future.await) });
                let tokio_id = abort.id();
                active_by_id.insert(task_id, tokio_id);
                active_by_tokio.insert(tokio_id, ActiveTask { id: task_id, request_type, abort });
            }
            message = control_receiver.recv() => {
                match message {
                    Some(ControlMessage::Cancel(id)) => {
                        if let Some(tokio_id) = active_by_id.remove(&id) {
                            if let Some(task) = active_by_tokio.remove(&tokio_id) {
                                task.abort.abort();
                                send_completion(
                                    &completion_sender,
                                    Completion::Failed(cancelled_failure(id, task.request_type)),
                                    control.as_ref(),
                                );
                            }
                        } else if id.get() > highest_seen_id {
                            pending_cancellations.insert(id);
                        }
                    }
                    Some(ControlMessage::Shutdown) | None => {
                        abort_all(&mut tasks).await;
                        break;
                    }
                }
            }
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                match result {
                    Some(Ok((tokio_id, (task_id, outcome)))) => {
                        active_by_id.remove(&task_id);
                        active_by_tokio.remove(&tokio_id);
                        let completion = match outcome {
                            Ok(command) => Completion::Apply(command),
                            Err(failure) => Completion::Failed(failure),
                        };
                        if !send_completion(&completion_sender, completion, control.as_ref()) {
                            abort_all(&mut tasks).await;
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        let tokio_id = error.id();
                        if let Some(task) = active_by_tokio.remove(&tokio_id) {
                            active_by_id.remove(&task.id);
                            send_completion(
                                &completion_sender,
                                Completion::Failed(AsyncTaskFailed {
                                    task_id: task.id,
                                    request_type: task.request_type,
                                    kind: AsyncTaskFailureKind::Panic,
                                    message: error.to_string(),
                                }),
                                control.as_ref(),
                            );
                        }
                    }
                    None => {}
                }
            }
        }
    }
}

fn cancelled_failure(id: AsyncTaskId, request_type: &'static str) -> AsyncTaskFailed {
    AsyncTaskFailed {
        task_id: id,
        request_type,
        kind: AsyncTaskFailureKind::Cancelled,
        message: "task was cancelled".into(),
    }
}

fn send_completion(
    sender: &mpsc::Sender<Completion>,
    completion: Completion,
    control: Option<&AppControl>,
) -> bool {
    if sender.send(completion).is_err() {
        return false;
    }
    if let Some(control) = control {
        control.wake();
    }
    true
}

async fn abort_all(tasks: &mut JoinSet<(AsyncTaskId, TaskResult)>) {
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}
