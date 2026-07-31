use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use core_plugin::Event;

use crate::AsyncTaskError;

pub(crate) type ErasedFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'static>>;
pub(crate) type ErasedAsyncTask<T, E> = Box<dyn FnOnce() -> ErasedFuture<T, E> + Send + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncRequestMode {
    Normal,
    BlockNextFrame,
}

pub struct AsyncRequest<T, E> {
    task: Mutex<Option<ErasedAsyncTask<T, E>>>,
    mode: AsyncRequestMode,
}

impl<T, E> AsyncRequest<T, E>
where
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
{
    pub fn new<Task, TaskFuture>(task: Task) -> Self
    where
        Task: FnOnce() -> TaskFuture + Send + 'static,
        TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
    {
        Self::from_task(task, AsyncRequestMode::Normal)
    }

    pub fn blocking<Task, TaskFuture>(task: Task) -> Self
    where
        Task: FnOnce() -> TaskFuture + Send + 'static,
        TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
    {
        Self::from_task(task, AsyncRequestMode::BlockNextFrame)
    }

    fn from_task<Task, TaskFuture>(task: Task, mode: AsyncRequestMode) -> Self
    where
        Task: FnOnce() -> TaskFuture + Send + 'static,
        TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
    {
        Self {
            task: Mutex::new(Some(Box::new(move || Box::pin(task())))),
            mode,
        }
    }

    pub(crate) fn take_task(&self) -> ErasedAsyncTask<T, E> {
        self.task
            .lock()
            .expect("async request task lock poisoned")
            .take()
            .expect("async request task has already been taken")
    }

    pub(crate) fn mode(&self) -> AsyncRequestMode {
        self.mode
    }
}

impl<T, E> Event for AsyncRequest<T, E>
where
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
{
}
