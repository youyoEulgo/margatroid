use std::any::type_name;

use app_runtime_plugin::{RuntimeHandle, PRE_UPDATE};
use core_plugin::{App, Plugin, World};

use crate::request::ErasedAsyncTask;
use crate::resource::{AsyncExecutorHandle, AsyncRequestRegistry};
use crate::runtime::start_executor;
use crate::{AsyncRequest, AsyncRequestMode, AsyncRuntimeError, AsyncTaskError};

#[derive(Clone, Copy, Debug, Default)]
pub struct AsyncRuntimePlugin;

impl Plugin for AsyncRuntimePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            AsyncRuntimeError::RuntimePluginMissing.panic();
        }
        if app.world().contains_resource::<AsyncExecutorHandle>()
            || app.world().contains_resource::<AsyncRequestRegistry>()
        {
            AsyncRuntimeError::AsyncRuntimePluginAlreadyInstalled.panic();
        }

        let executor = start_executor();
        app.world_mut().insert_resource(executor);
        app.world_mut().insert_resource(AsyncRequestRegistry::new());
    }
}

pub trait AppAsyncExt {
    fn register_async_request<T, E>(&mut self) -> &mut Self
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static;

    fn register_async_request_in<T, E>(&mut self, schedule: &str) -> &mut Self
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static;
}

impl AppAsyncExt for App {
    fn register_async_request<T, E>(&mut self) -> &mut Self
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
    {
        self.register_async_request_in::<T, E>(PRE_UPDATE)
    }

    fn register_async_request_in<T, E>(&mut self, schedule: &str) -> &mut Self
    where
        T: Send + Sync + 'static,
        E: From<AsyncTaskError> + Send + Sync + 'static,
    {
        let Some(registry) = self.world_mut().get_resource_mut::<AsyncRequestRegistry>() else {
            AsyncRuntimeError::AsyncRuntimePluginMissing.panic();
        };
        if !registry.register::<T, E>() {
            AsyncRuntimeError::RequestAlreadyRegistered {
                request_type: type_name::<AsyncRequest<T, E>>(),
            }
            .panic();
        }

        self.register_event::<AsyncRequest<T, E>>()
            .register_event::<Result<T, E>>()
            .add_system(schedule, dispatch_async_requests::<T, E>)
    }
}

fn dispatch_async_requests<T, E>(world: &mut World)
where
    T: Send + Sync + 'static,
    E: From<AsyncTaskError> + Send + Sync + 'static,
{
    let runtime = world
        .get_resource::<RuntimeHandle>()
        .unwrap_or_else(|| AsyncRuntimeError::RuntimePluginMissing.panic())
        .clone();
    let requests = world
        .event_reader::<AsyncRequest<T, E>>()
        .into_iter()
        .map(|request| (request.take_task(), request.mode()))
        .collect::<Vec<(ErasedAsyncTask<T, E>, AsyncRequestMode)>>();

    for (task, mode) in requests {
        let event_handle = world.event_write().send_pending::<T, E>();
        if mode == AsyncRequestMode::BlockNextFrame {
            runtime.close_gate();
        }
        let task_runtime = runtime.clone();
        let supervised = async move {
            let result = match tokio::spawn(async move { task().await }).await {
                Ok(result) => result,
                Err(error) => Err(E::from(AsyncTaskError::from_join_error(error))),
            };
            event_handle.complete(result);
            match mode {
                AsyncRequestMode::Normal => task_runtime.wake(),
                AsyncRequestMode::BlockNextFrame => task_runtime.open_gate(),
            }
        };
        world
            .get_resource::<AsyncExecutorHandle>()
            .unwrap_or_else(|| AsyncRuntimeError::AsyncRuntimePluginMissing.panic())
            .spawn(Box::pin(supervised));
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::time::{Duration, Instant};

    use app_runtime_plugin::RuntimePlugin;

    use super::*;

    #[derive(Debug)]
    enum TestError {
        Business,
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
                Self::Business => formatter.write_str("business error"),
                Self::Async(error) => error.fmt(formatter),
            }
        }
    }

    impl std::error::Error for TestError {}

    fn app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .register_async_request::<u32, TestError>();
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
                return match result {
                    Ok(value) => Ok(*value),
                    Err(TestError::Business) => Err(TestError::Business),
                    Err(TestError::Async(AsyncTaskError::Panicked { message })) => {
                        Err(TestError::Async(AsyncTaskError::Panicked {
                            message: message.clone(),
                        }))
                    }
                    Err(TestError::Async(AsyncTaskError::Cancelled)) => {
                        Err(TestError::Async(AsyncTaskError::Cancelled))
                    }
                };
            }
            assert!(Instant::now() < deadline, "async response timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn successful_task_completes_the_pending_response() {
        let mut app = app();
        app.world()
            .event_write()
            .send_event(AsyncRequest::<u32, TestError>::new(|| async { Ok(42) }));

        assert!(matches!(wait_for_response(&mut app), Ok(42)));
    }

    #[test]
    fn business_error_remains_the_developer_error() {
        let mut app = app();
        app.world()
            .event_write()
            .send_event(AsyncRequest::<u32, TestError>::new(|| async {
                Err(TestError::Business)
            }));

        assert!(matches!(
            wait_for_response(&mut app),
            Err(TestError::Business)
        ));
    }

    #[test]
    fn panicked_task_completes_with_an_async_task_error() {
        let mut app = app();
        app.world()
            .event_write()
            .send_event(AsyncRequest::<u32, TestError>::new(|| async {
                panic!("async boom")
            }));

        assert!(matches!(
            wait_for_response(&mut app),
            Err(TestError::Async(AsyncTaskError::Panicked { message }))
                if message == "async boom"
        ));
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn duplicate_request_registration_is_rejected() {
        app().register_async_request::<u32, TestError>();
    }

    #[test]
    #[should_panic(expected = "RuntimePlugin is not installed")]
    fn runtime_plugin_must_be_installed_first() {
        App::new().add_plugin(AsyncRuntimePlugin);
    }

    #[test]
    #[should_panic(expected = "AsyncRuntimePlugin is already installed")]
    fn async_runtime_plugin_cannot_be_installed_twice() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(AsyncRuntimePlugin);
    }

    #[test]
    fn dropping_app_cancels_pending_tasks_and_joins_the_executor() {
        let start = Instant::now();
        {
            let mut app = app();
            app.world()
                .event_write()
                .send_event(AsyncRequest::<u32, TestError>::new(|| {
                    std::future::pending()
                }));
            app.tick();
        }

        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
