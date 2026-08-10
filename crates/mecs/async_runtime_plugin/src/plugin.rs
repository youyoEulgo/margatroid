use std::any::type_name;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, WorldEventExt};
use core_plugin::{App, Event, EventHandle, Plugin, System, World};

use crate::request::{AsyncEventRequest, AsyncMode};
use crate::resource::{AsyncRegistry, AsyncRuntimeHandle};
use crate::runtime::start_executor;
use crate::{AsyncContext, AsyncEventHandler, AsyncRuntimeError, AsyncTask, AsyncTaskError};

#[derive(Clone, Copy, Debug, Default)]
pub struct AsyncRuntimePlugin;

impl Plugin for AsyncRuntimePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            AsyncRuntimeError::RuntimePluginMissing.panic();
        }
        if app.world().contains_resource::<AsyncRuntimeHandle>()
            || app.world().contains_resource::<AsyncRegistry>()
        {
            AsyncRuntimeError::AsyncRuntimePluginAlreadyInstalled.panic();
        }

        let executor = start_executor();
        app.world_mut().insert_resource(executor);
        app.world_mut().insert_resource(AsyncRegistry::new());
    }
}

pub trait AppAsyncExt {
    fn add_async_system<Request, T, E, Handler, Args: 'static>(
        &mut self,
        schedule: &str,
        handler: Handler,
    ) -> &mut Self
    where
        Request: Event,
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Handler: AsyncEventHandler<Request, T, E, Args>;
}

impl AppAsyncExt for App {
    fn add_async_system<Request, T, E, Handler, Args: 'static>(
        &mut self,
        schedule: &str,
        handler: Handler,
    ) -> &mut Self
    where
        Request: Event,
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
        Handler: AsyncEventHandler<Request, T, E, Args>,
    {
        let Some(registry) = self.world_mut().get_resource_mut::<AsyncRegistry>() else {
            AsyncRuntimeError::AsyncRuntimePluginMissing.panic();
        };
        if !registry.register_event_system::<Request>() {
            AsyncRuntimeError::AsyncSystemAlreadyRegistered {
                event_type: type_name::<Request>(),
            }
            .panic();
        }

        self.add_system(
            schedule,
            AsyncEventSystem::<Request, T, E, Handler, Args>::new(handler),
        )
    }
}

struct AsyncEventSystem<Request, T, E, Handler, Args> {
    handler: Arc<Handler>,
    marker: PhantomData<fn(Request, T, E, Args)>,
}

impl<Request, T, E, Handler, Args> AsyncEventSystem<Request, T, E, Handler, Args> {
    fn new(handler: Handler) -> Self {
        Self {
            handler: Arc::new(handler),
            marker: PhantomData,
        }
    }
}

impl<Request, T, E, Handler, Args: 'static> System
    for AsyncEventSystem<Request, T, E, Handler, Args>
where
    Request: Event,
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
    Handler: AsyncEventHandler<Request, T, E, Args>,
{
    fn run(&mut self, world: &mut World) {
        let requests = world
            .event_reader::<AsyncEventRequest<Request>>()
            .into_iter()
            .filter_map(|request| request.take_event().map(|event| (event, request.mode())))
            .collect::<Vec<_>>();

        for (request, mode) in requests {
            submit_event_task(world, request, mode, Arc::clone(&self.handler));
        }
    }
}

fn submit_event_task<Request, T, E, Handler, Args>(
    world: &mut World,
    request: Request,
    mode: AsyncMode,
    handler: Arc<Handler>,
) where
    Request: Event,
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
    Handler: AsyncEventHandler<Request, T, E, Args>,
{
    let runtime = runtime_handle(world);
    let context = AsyncContext::new(world.event_sender());
    let event_handle = world.emit_pending::<T, E>();
    if mode == AsyncMode::Await {
        runtime.close_gate();
    }
    let future = handler.run(request, context);
    let executor = async_runtime_handle(world);
    submit_supervised(executor, runtime, event_handle, mode, future);
}

pub(crate) fn dispatch_closure_task<T, E, Task, Args>(
    world: &mut World,
    task: Task,
    mode: AsyncMode,
) where
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
    Task: AsyncTask<T, E, Args>,
{
    let runtime = runtime_handle(world);
    let context = AsyncContext::new(world.event_sender());
    let event_handle = world.emit_pending::<T, E>();
    if mode == AsyncMode::Await {
        runtime.close_gate();
    }
    let future = task.run(context);
    let executor = async_runtime_handle(world);
    submit_supervised(executor, runtime, event_handle, mode, future);
}

fn submit_supervised<T, E, TaskFuture>(
    executor: &AsyncRuntimeHandle,
    runtime: RuntimeHandle,
    event_handle: EventHandle<Result<T, E>>,
    mode: AsyncMode,
    future: TaskFuture,
) where
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
    TaskFuture: Future<Output = Result<T, E>> + Send + 'static,
{
    let supervised = async move {
        let result = match tokio::spawn(future).await {
            Ok(result) => result,
            Err(error) => Err(E::from(AsyncTaskError::from_join_error(error))),
        };
        event_handle.complete(result);
        match mode {
            AsyncMode::Async => runtime.wake(),
            AsyncMode::Await => runtime.open_gate(),
        }
    };
    executor.spawn(Box::pin(supervised));
}

fn runtime_handle(world: &World) -> RuntimeHandle {
    world
        .get_resource::<RuntimeHandle>()
        .unwrap_or_else(|| AsyncRuntimeError::RuntimePluginMissing.panic())
        .clone()
}

fn async_runtime_handle(world: &World) -> &AsyncRuntimeHandle {
    world
        .get_resource::<AsyncRuntimeHandle>()
        .unwrap_or_else(|| AsyncRuntimeError::AsyncRuntimePluginMissing.panic())
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::time::{Duration, Instant};

    use app_runtime_plugin::RuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};

    use super::*;
    use crate::WorldAsyncExt;

    #[derive(Debug)]
    enum TestError {
        Async(AsyncTaskError),
    }

    impl From<AsyncTaskError> for TestError {
        fn from(error: AsyncTaskError) -> Self {
            Self::Async(error)
        }
    }

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Async(error) => error.fmt(formatter),
            }
        }
    }

    impl std::error::Error for TestError {}

    struct Calculate(u32);
    impl Event for Calculate {}

    struct Progress(u32);
    impl Event for Progress {}

    async fn calculate(request: Calculate, context: AsyncContext) -> Result<u32, TestError> {
        context.send_event(Progress(request.0));
        Ok(request.0 * 2)
    }

    fn event_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_async_system(RuntimePlugin::PRE_UPDATE, calculate);
        app
    }

    fn closure_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::PRE_UPDATE)
            .add_plugin(AsyncRuntimePlugin);
        app
    }

    fn wait_for_response(app: &mut App) -> Result<u32, TestError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(result) = app
                .world()
                .event_reader::<Result<u32, TestError>>()
                .into_iter()
                .next()
            {
                return clone_result(result);
            }
            assert!(Instant::now() < deadline, "async response timed out");
            std::thread::yield_now();
        }
    }

    fn clone_result(result: &Result<u32, TestError>) -> Result<u32, TestError> {
        match result {
            Ok(value) => Ok(*value),
            Err(TestError::Async(AsyncTaskError::Panicked { message })) => {
                Err(TestError::Async(AsyncTaskError::Panicked {
                    message: message.clone(),
                }))
            }
            Err(TestError::Async(AsyncTaskError::Cancelled)) => {
                Err(TestError::Async(AsyncTaskError::Cancelled))
            }
        }
    }

    #[test]
    fn event_mode_uses_the_registered_handler() {
        let mut app = event_app();
        app.world().send_async_event(Calculate(21));

        assert!(matches!(wait_for_response(&mut app), Ok(42)));
    }

    #[test]
    fn await_event_mode_completes_its_response() {
        let mut app = event_app();
        app.world().send_await_event(Calculate(3));

        assert!(matches!(wait_for_response(&mut app), Ok(6)));
    }

    #[test]
    fn event_handler_receives_an_injected_context() {
        let mut app = event_app();
        app.world().send_async_event(Calculate(25));
        app.tick();

        let deadline = Instant::now() + Duration::from_secs(2);
        while app.world().event_snapshot().normal_event_count < 2 {
            assert!(Instant::now() < deadline, "async events timed out");
            std::thread::yield_now();
        }
        app.tick();

        assert_eq!(
            app.world()
                .event_reader::<Progress>()
                .into_iter()
                .next()
                .unwrap()
                .0,
            25
        );
    }

    #[test]
    fn closure_mode_runs_a_one_shot_async_task() {
        let mut app = closure_app();
        app.world()
            .send_async_closure(RuntimePlugin::PRE_UPDATE, || async {
                Ok::<u32, TestError>(7)
            });

        assert!(matches!(wait_for_response(&mut app), Ok(7)));
    }

    #[test]
    fn await_closure_mode_completes_its_response() {
        let mut app = closure_app();
        app.world()
            .send_await_closure(RuntimePlugin::PRE_UPDATE, || async {
                Ok::<u32, TestError>(9)
            });

        assert!(matches!(wait_for_response(&mut app), Ok(9)));
    }

    #[test]
    fn panicked_closure_completes_with_an_async_task_error() {
        let mut app = closure_app();
        app.world()
            .send_async_closure::<u32, TestError, _, _>(RuntimePlugin::PRE_UPDATE, || async {
                panic!("async boom")
            });

        assert!(matches!(
            wait_for_response(&mut app),
            Err(TestError::Async(AsyncTaskError::Panicked { message }))
                if message == "async boom"
        ));
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn duplicate_async_system_is_rejected() {
        event_app().add_async_system(RuntimePlugin::PRE_UPDATE, calculate);
    }

    #[test]
    #[should_panic(expected = "no async system is registered")]
    fn sending_an_unregistered_event_is_rejected() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin);
        app.world().send_async_event(Calculate(1));
    }

    #[test]
    #[should_panic(expected = "RuntimePlugin is not installed")]
    fn runtime_plugin_must_be_installed_first() {
        App::new().add_plugin(AsyncRuntimePlugin);
    }

    #[test]
    fn dropping_app_cancels_pending_tasks_and_joins_the_executor() {
        let start = Instant::now();
        {
            let mut app = closure_app();
            app.world().send_async_closure(
                RuntimePlugin::PRE_UPDATE,
                std::future::pending::<Result<u32, TestError>>,
            );
            app.tick();
        }

        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
