use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use app_runtime_plugin::WorldEventExt;
use core_plugin::{Event, World};

use crate::{AsyncContext, AsyncTaskError};

pub(crate) type ErasedFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'static>>;
pub(crate) type ErasedAsyncTask<T, E> =
    Box<dyn FnOnce(AsyncContext) -> ErasedFuture<T, E> + Send + 'static>;

pub trait AsyncTask<T, E, Args>: Send + 'static {
    type Future: Future<Output = Result<T, E>> + Send + 'static;

    fn run(self, context: AsyncContext) -> Self::Future;
}

impl<T, E, Task, TaskFuture> AsyncTask<T, E, ()> for Task
where
    Task: FnOnce() -> TaskFuture + Send + 'static,
    TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
{
    type Future = TaskFuture;

    fn run(self, _context: AsyncContext) -> Self::Future {
        self()
    }
}

impl<T, E, Task, TaskFuture> AsyncTask<T, E, (AsyncContext,)> for Task
where
    Task: FnOnce(AsyncContext) -> TaskFuture + Send + 'static,
    TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
{
    type Future = TaskFuture;

    fn run(self, context: AsyncContext) -> Self::Future {
        self(context)
    }
}

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
    pub fn new<Task, Args>(task: Task) -> Self
    where
        Task: AsyncTask<T, E, Args>,
    {
        Self::from_task(task, AsyncRequestMode::Normal)
    }

    pub fn blocking<Task, Args>(task: Task) -> Self
    where
        Task: AsyncTask<T, E, Args>,
    {
        Self::from_task(task, AsyncRequestMode::BlockNextFrame)
    }

    fn from_task<Task, Args>(task: Task, mode: AsyncRequestMode) -> Self
    where
        Task: AsyncTask<T, E, Args>,
    {
        Self {
            task: Mutex::new(Some(Box::new(move |context| Box::pin(task.run(context))))),
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

pub trait WorldAsyncExt {
    fn send_async_event<T, E, Task, Args>(&self, task: Task, blocking: bool)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>;
}

impl WorldAsyncExt for World {
    fn send_async_event<T, E, Task, Args>(&self, task: Task, blocking: bool)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>,
    {
        let request = if blocking {
            AsyncRequest::blocking(task)
        } else {
            AsyncRequest::new(task)
        };
        self.send_event(request);
    }
}

#[cfg(test)]
mod tests {
    use core_plugin::App;

    use super::*;

    async fn value_after(value: u32) -> Result<u32, TestError> {
        Ok(value)
    }

    struct TestError;

    impl From<AsyncTaskError> for TestError {
        fn from(_error: AsyncTaskError) -> Self {
            Self
        }
    }

    #[test]
    fn world_builds_normal_and_blocking_async_events() {
        let mut app = App::new();
        app.register_event::<AsyncRequest<u32, TestError>>()
            .add_plugin(app_runtime_plugin::RuntimePlugin::default());

        let value = 1;
        app.world()
            .send_async_event(move || value_after(value), false);
        app.tick();
        assert_eq!(
            app.world()
                .event_reader::<AsyncRequest<u32, TestError>>()
                .into_iter()
                .next()
                .unwrap()
                .mode(),
            AsyncRequestMode::Normal
        );

        app.world()
            .send_async_event(|| async { Ok::<u32, TestError>(2) }, true);
        app.tick();
        assert_eq!(
            app.world()
                .event_reader::<AsyncRequest<u32, TestError>>()
                .into_iter()
                .next()
                .unwrap()
                .mode(),
            AsyncRequestMode::BlockNextFrame
        );
    }

    #[test]
    #[should_panic(expected = "RuntimePlugin is not installed")]
    fn sending_an_async_event_requires_the_runtime_plugin() {
        let mut app = App::new();
        app.register_event::<AsyncRequest<u32, TestError>>();
        app.world()
            .send_async_event(|| async { Ok::<u32, TestError>(1) }, false);
    }
}
