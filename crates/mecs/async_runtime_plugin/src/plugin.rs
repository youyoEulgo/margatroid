use std::any::TypeId;
use std::future::Future;

use app_runtime_plugin::AppControl;
use core_plugin::{named_system, App, Event, Plugin, Stage, World};

use crate::runtime::{AsyncRuntimeState, Completion};
use crate::{
    AsyncRuntimeOptions, AsyncSystemOptions, AsyncTaskControl, AsyncTaskFailed, AsyncTaskId,
    AsyncTaskStarted,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct AsyncRuntimePlugin {
    options: AsyncRuntimeOptions,
}

impl AsyncRuntimePlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_queue_capacity(mut self, capacity: usize) -> Self {
        assert!(capacity > 0, "async queue capacity must be positive");
        self.options.queue_capacity = capacity;
        self
    }

    pub fn with_max_in_flight(mut self, limit: usize) -> Self {
        assert!(limit > 0, "max in-flight tasks must be positive");
        self.options.max_in_flight = limit;
        self
    }
}

impl Plugin for AsyncRuntimePlugin {
    fn build(&self, app: &mut App) {
        if app.world().resource::<AsyncRuntimeState>().is_some() {
            return;
        }
        app.add_event::<AsyncTaskStarted>();
        app.add_event::<AsyncTaskFailed>();
        app.add_resource(AsyncRuntimeState::new(self.options));
        app.add_systems(
            Stage::Startup,
            [named_system("async_runtime.start", start_runtime)],
        );
        app.add_systems(
            Stage::First,
            [named_system("async_runtime.collect", collect_completions)],
        );
    }
}

fn start_runtime(world: &mut World) {
    let control = world.resource::<AppControl>().cloned();
    let task_control = world
        .resource::<AsyncRuntimeState>()
        .expect("AsyncRuntimeState should be registered")
        .start(control);
    if let Some(task_control) = task_control {
        world.add_resource(task_control);
    }
}

fn collect_completions(world: &mut World) {
    let completions = world
        .resource::<AsyncRuntimeState>()
        .expect("AsyncRuntimeState should be registered")
        .drain_completions();
    for completion in completions {
        match completion {
            Completion::Apply(command) => command(world),
            Completion::Failed(failure) => world.send_event(failure),
        }
    }
}

pub trait AsyncAppExt {
    fn add_async_system<Request, Output, Handler, Fut>(&mut self, handler: Handler) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static;

    fn add_async_system_with_options<Request, Output, Handler, Fut>(
        &mut self,
        handler: Handler,
        options: AsyncSystemOptions,
    ) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static;
}

impl AsyncAppExt for App {
    fn add_async_system<Request, Output, Handler, Fut>(&mut self, handler: Handler) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static,
    {
        self.add_async_system_with_options(handler, AsyncSystemOptions::default())
    }

    fn add_async_system_with_options<Request, Output, Handler, Fut>(
        &mut self,
        mut handler: Handler,
        options: AsyncSystemOptions,
    ) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static,
    {
        assert!(
            self.world().resource::<AsyncRuntimeState>().is_some(),
            "AsyncRuntimePlugin must be installed before registering async systems"
        );
        assert_ne!(
            TypeId::of::<Request>(),
            TypeId::of::<Output>(),
            "async request and output event types must be different"
        );
        self.add_event::<Request>();
        self.add_event::<Output>();
        self.add_event::<AsyncTaskStarted>();
        self.add_event::<AsyncTaskFailed>();

        let mut reader = self.event_reader::<Request>();
        let request_type = std::any::type_name::<Request>();
        self.add_systems(
            Stage::Last,
            [move |world: &mut World| {
                let requests = world.read_events(&mut reader);
                for request in requests {
                    let spawner = world
                        .resource::<AsyncRuntimeState>()
                        .and_then(AsyncRuntimeState::spawner);
                    let Some(spawner) = spawner else {
                        world.send_event(AsyncTaskFailed {
                            task_id: AsyncTaskId(0),
                            request_type,
                            kind: crate::AsyncTaskFailureKind::WorkerStopped,
                            message: "async worker has not started".into(),
                        });
                        continue;
                    };
                    match spawner.spawn(handler(request), request_type, options) {
                        Ok(task_id) => world.send_event(AsyncTaskStarted {
                            task_id,
                            request_type,
                        }),
                        Err(failure) => world.send_event(failure),
                    }
                }
            }],
        );
        self
    }
}

pub trait AsyncWorldExt {
    fn cancel_async_task(&self, id: AsyncTaskId) -> bool;
}

impl AsyncWorldExt for World {
    fn cancel_async_task(&self, id: AsyncTaskId) -> bool {
        self.resource::<AsyncTaskControl>()
            .is_some_and(|control| control.cancel(id))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::AsyncTaskFailureKind;

    #[derive(Clone)]
    struct DoubleRequest(i32);
    #[derive(Clone, Debug, PartialEq)]
    struct DoubleOutput(i32);

    #[derive(Clone)]
    struct PendingRequest;
    #[derive(Clone)]
    struct PendingOutput;

    #[test]
    fn request_runs_on_worker_and_returns_as_event() {
        let mut app = App::new();
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_async_system(|request: DoubleRequest| async move { DoubleOutput(request.0 * 2) });
        let mut output_reader = app.event_reader::<DoubleOutput>();

        app.world().send_event(DoubleRequest(21));
        app.tick();
        let outputs = tick_until(
            &mut app,
            || false,
            |app| app.world().read_events(&mut output_reader),
        );

        assert_eq!(outputs, [DoubleOutput(42)]);
    }

    #[test]
    fn timeout_is_reported() {
        let mut app = App::new();
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_async_system_with_options(
            |_request: DoubleRequest| async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                DoubleOutput(0)
            },
            AsyncSystemOptions {
                timeout: Some(Duration::from_millis(1)),
            },
        );
        let mut failure_reader = app.event_reader::<AsyncTaskFailed>();

        app.world().send_event(DoubleRequest(1));
        app.tick();
        let failures = tick_until(
            &mut app,
            || false,
            |app| app.world().read_events(&mut failure_reader),
        );

        assert_eq!(failures[0].kind, AsyncTaskFailureKind::Timeout);
    }

    #[test]
    fn panic_is_reported() {
        let mut app = App::new();
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_async_system::<PendingRequest, PendingOutput, _, _>(|_request| async {
            panic!("async boom")
        });
        let mut failure_reader = app.event_reader::<AsyncTaskFailed>();

        app.world().send_event(PendingRequest);
        app.tick();
        let failures = tick_until(
            &mut app,
            || false,
            |app| app.world().read_events(&mut failure_reader),
        );

        assert_eq!(failures[0].kind, AsyncTaskFailureKind::Panic);
    }

    #[test]
    fn task_can_be_cancelled_from_main_thread() {
        let mut app = App::new();
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_async_system_with_options(
            |_request: PendingRequest| std::future::pending::<PendingOutput>(),
            AsyncSystemOptions { timeout: None },
        );
        let mut started_reader = app.event_reader::<AsyncTaskStarted>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                for started in world.read_events(&mut started_reader) {
                    assert!(world.cancel_async_task(started.task_id));
                }
            }],
        );
        let mut failure_reader = app.event_reader::<AsyncTaskFailed>();

        app.world().send_event(PendingRequest);
        app.tick();
        let failures = tick_until(
            &mut app,
            || false,
            |app| app.world().read_events(&mut failure_reader),
        );

        assert_eq!(failures[0].kind, AsyncTaskFailureKind::Cancelled);
    }

    #[test]
    fn bounded_queue_reports_backpressure() {
        let mut app = App::new();
        app.add_plugins(
            AsyncRuntimePlugin::new()
                .with_queue_capacity(1)
                .with_max_in_flight(1),
        );
        app.add_async_system_with_options(
            |_request: PendingRequest| std::future::pending::<PendingOutput>(),
            AsyncSystemOptions { timeout: None },
        );
        let mut failure_reader = app.event_reader::<AsyncTaskFailed>();
        for _ in 0..100 {
            app.world().send_event(PendingRequest);
        }

        app.tick();
        let failures = app.world().read_events(&mut failure_reader);

        assert!(failures
            .iter()
            .any(|failure| failure.kind == AsyncTaskFailureKind::QueueFull));
    }

    #[test]
    fn dropping_app_cancels_pending_tasks() {
        let start = Instant::now();
        {
            let mut app = App::new();
            app.add_plugins(AsyncRuntimePlugin::default());
            app.add_async_system_with_options(
                |_request: PendingRequest| std::future::pending::<PendingOutput>(),
                AsyncSystemOptions { timeout: None },
            );
            app.world().send_event(PendingRequest);
            app.tick();
        }

        assert!(start.elapsed() < Duration::from_secs(1));
    }

    fn tick_until<T>(
        app: &mut App,
        mut stop: impl FnMut() -> bool,
        mut read: impl FnMut(&App) -> Vec<T>,
    ) -> Vec<T> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            let values = read(app);
            if !values.is_empty() || stop() {
                return values;
            }
            assert!(Instant::now() < deadline, "async result timed out");
            std::thread::yield_now();
        }
    }
}
