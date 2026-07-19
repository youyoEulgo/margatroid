use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc as tokio_mpsc;
use tokio::task::{AbortHandle, Id as TokioTaskId, JoinSet};

use crate::events::Event;
use crate::system::System;
use crate::world::World;
use crate::AppControl;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

type WorldCommand = Box<dyn FnOnce(&mut World) + Send + 'static>;
type TaskResult = Result<WorldCommand, AsyncTaskFailed>;
type AsyncTaskFuture = Pin<Box<dyn Future<Output = TaskResult> + Send + 'static>>;

/// core_plugin 分配的异步任务标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AsyncTaskId(u64);

impl AsyncTaskId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncTaskFailureKind {
    QueueFull,
    WorkerStopped,
    Timeout,
    Cancelled,
    Panic,
}

/// 任务成功提交到异步线程后产生。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncTaskStarted {
    pub task_id: AsyncTaskId,
    pub request_type: &'static str,
}

/// 框架无法产生正常 Output 时产生。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncTaskFailed {
    pub task_id: AsyncTaskId,
    pub request_type: &'static str,
    pub kind: AsyncTaskFailureKind,
    pub message: String,
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

struct AsyncTask {
    id: AsyncTaskId,
    request_type: &'static str,
    future: AsyncTaskFuture,
}

enum ControlMessage {
    Cancel(AsyncTaskId),
    Shutdown,
}

enum Completion {
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
        let task = AsyncTask {
            id,
            request_type,
            future: task_future,
        };

        self.task_sender.try_send(task).map_err(|error| {
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

    fn cancel(&self, id: AsyncTaskId) -> bool {
        self.control_sender.send(ControlMessage::Cancel(id)).is_ok()
    }
}

#[derive(Clone)]
pub(crate) struct AsyncTaskControl {
    spawner: AsyncSpawner,
}

impl AsyncTaskControl {
    pub(crate) fn cancel(&self, id: AsyncTaskId) -> bool {
        self.spawner.cancel(id)
    }
}

pub(crate) struct AsyncCompletionSystem {
    receiver: mpsc::Receiver<Completion>,
}

impl System for AsyncCompletionSystem {
    fn run(&mut self, world: &mut World) {
        for completion in self.receiver.try_iter() {
            match completion {
                Completion::Apply(command) => command(world),
                Completion::Failed(failure) => world.send_event(failure),
            }
        }
    }
}

pub(crate) struct AsyncWorker {
    spawner: AsyncSpawner,
    thread: Option<thread::JoinHandle<()>>,
}

impl AsyncWorker {
    pub(crate) fn start(
        control: AppControl,
        queue_capacity: usize,
        max_in_flight: usize,
    ) -> (Self, AsyncCompletionSystem) {
        assert!(queue_capacity > 0, "async queue capacity must be positive");
        assert!(max_in_flight > 0, "max in-flight tasks must be positive");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build async worker runtime");
        let (task_sender, task_receiver) = tokio_mpsc::channel(queue_capacity);
        let (control_sender, control_receiver) = tokio_mpsc::unbounded_channel();
        let (completion_sender, completion_receiver) = mpsc::channel();
        let spawner = AsyncSpawner {
            task_sender,
            control_sender,
            next_id: Arc::new(AtomicU64::new(1)),
        };
        let thread = thread::Builder::new()
            .name("core-plugin-async".into())
            .spawn(move || {
                runtime.block_on(worker_loop(
                    task_receiver,
                    control_receiver,
                    completion_sender,
                    control,
                    max_in_flight,
                ));
            })
            .expect("failed to start async worker thread");

        (
            Self {
                spawner,
                thread: Some(thread),
            },
            AsyncCompletionSystem {
                receiver: completion_receiver,
            },
        )
    }

    pub(crate) fn spawner(&self) -> AsyncSpawner {
        self.spawner.clone()
    }

    pub(crate) fn task_control(&self) -> AsyncTaskControl {
        AsyncTaskControl {
            spawner: self.spawner(),
        }
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
    control: AppControl,
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
                        &control,
                    );
                    continue;
                }
                let task_id = task.id;
                let request_type = task.request_type;
                let abort = tasks.spawn(async move { (task_id, task.future.await) });
                let tokio_id = abort.id();
                active_by_id.insert(task_id, tokio_id);
                active_by_tokio.insert(tokio_id, ActiveTask {
                    id: task_id,
                    request_type,
                    abort,
                });
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
                                    &control,
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
                        if !send_completion(&completion_sender, completion, &control) {
                            abort_all(&mut tasks).await;
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        let tokio_id = error.id();
                        if let Some(task) = active_by_tokio.remove(&tokio_id) {
                            active_by_id.remove(&task.id);
                            let failure = AsyncTaskFailed {
                                task_id: task.id,
                                request_type: task.request_type,
                                kind: AsyncTaskFailureKind::Panic,
                                message: error.to_string(),
                            };
                            send_completion(
                                &completion_sender,
                                Completion::Failed(failure),
                                &control,
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
    control: &AppControl,
) -> bool {
    if sender.send(completion).is_err() {
        return false;
    }
    control.wake();
    true
}

async fn abort_all(tasks: &mut JoinSet<(AsyncTaskId, TaskResult)>) {
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}
