use std::any::type_name;
use std::future::Future;
use std::sync::Mutex;

use app_runtime_plugin::WorldEventExt;
use closure_plugin::WorldClosureExt;
use core_plugin::{Event, World};

use crate::plugin::dispatch_closure_task;
use crate::resource::AsyncRegistry;
use crate::{AsyncContext, AsyncRuntimeError, AsyncTaskError};

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

pub trait AsyncEventHandler<Request, T, E, Args>: Send + Sync + 'static {
    type Future: Future<Output = Result<T, E>> + Send + 'static;

    fn run(&self, request: Request, context: AsyncContext) -> Self::Future;
}

impl<Request, T, E, Handler, HandlerFuture> AsyncEventHandler<Request, T, E, ()> for Handler
where
    Handler: Fn(Request) -> HandlerFuture + Send + Sync + 'static,
    HandlerFuture: Future<Output = Result<T, E>> + Send + 'static,
{
    type Future = HandlerFuture;

    fn run(&self, request: Request, _context: AsyncContext) -> Self::Future {
        self(request)
    }
}

impl<Request, T, E, Handler, HandlerFuture> AsyncEventHandler<Request, T, E, (AsyncContext,)>
    for Handler
where
    Handler: Fn(Request, AsyncContext) -> HandlerFuture + Send + Sync + 'static,
    HandlerFuture: Future<Output = Result<T, E>> + Send + 'static,
{
    type Future = HandlerFuture;

    fn run(&self, request: Request, context: AsyncContext) -> Self::Future {
        self(request, context)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AsyncMode {
    Async,
    Await,
}

pub(crate) struct AsyncEventRequest<Request: Event> {
    event: Mutex<Option<Request>>,
    mode: AsyncMode,
}

impl<Request: Event> AsyncEventRequest<Request> {
    fn new(event: Request, mode: AsyncMode) -> Self {
        Self {
            event: Mutex::new(Some(event)),
            mode,
        }
    }

    pub(crate) fn take_event(&self) -> Option<Request> {
        self.event
            .lock()
            .expect("async event request lock poisoned")
            .take()
    }

    pub(crate) fn mode(&self) -> AsyncMode {
        self.mode
    }
}

impl<Request: Event> Event for AsyncEventRequest<Request> {}

pub trait WorldAsyncExt {
    fn send_async_event<Request: Event>(&self, event: Request);
    fn send_await_event<Request: Event>(&self, event: Request);

    fn send_async_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>;

    fn send_await_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>;

    fn spawn_async_service<Service>(&self, service: Service)
    where
        Service: Future<Output = ()> + Send + 'static;
}

impl WorldAsyncExt for World {
    fn send_async_event<Request: Event>(&self, event: Request) {
        ensure_async_system_registered::<Request>(self);
        WorldEventExt::send_event(self, AsyncEventRequest::new(event, AsyncMode::Async));
    }

    fn send_await_event<Request: Event>(&self, event: Request) {
        ensure_async_system_registered::<Request>(self);
        WorldEventExt::send_event(self, AsyncEventRequest::new(event, AsyncMode::Await));
    }

    fn send_async_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>,
    {
        WorldClosureExt::send_closure(self, schedule, move |world| {
            dispatch_closure_task::<T, E, Task, Args>(world, task, AsyncMode::Async);
        });
    }

    fn send_await_closure<T, E, Task, Args>(&self, schedule: &str, task: Task)
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Task: AsyncTask<T, E, Args>,
    {
        WorldClosureExt::send_closure(self, schedule, move |world| {
            dispatch_closure_task::<T, E, Task, Args>(world, task, AsyncMode::Await);
        });
    }

    fn spawn_async_service<Service>(&self, service: Service)
    where
        Service: Future<Output = ()> + Send + 'static,
    {
        self.get_resource::<crate::AsyncRuntimeHandle>()
            .unwrap_or_else(|| AsyncRuntimeError::AsyncRuntimePluginMissing.panic())
            .spawn(Box::pin(service));
    }
}

fn ensure_async_system_registered<Request: Event>(world: &World) {
    let Some(registry) = world.get_resource::<AsyncRegistry>() else {
        AsyncRuntimeError::AsyncRuntimePluginMissing.panic();
    };
    if !registry.contains_event_system::<Request>() {
        AsyncRuntimeError::AsyncSystemNotRegistered {
            event_type: type_name::<Request>(),
        }
        .panic();
    }
}
