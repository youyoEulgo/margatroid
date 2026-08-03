use std::time::{Duration, Instant};

use anyhow::anyhow;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AppAsyncExt, AsyncContext, AsyncRuntimePlugin, WorldAsyncExt};
use core_plugin::{App, Event};

struct LoadRequest;
impl Event for LoadRequest {}

async fn fail_to_load(_request: LoadRequest) -> Result<u32, anyhow::Error> {
    Err(anyhow!("business failure"))
}

struct ProgressRequest;
impl Event for ProgressRequest {}

struct Progress(u32);
impl Event for Progress {}

async fn report_progress(
    _request: ProgressRequest,
    context: AsyncContext,
) -> Result<u32, anyhow::Error> {
    context.send_event(Progress(50));
    Ok(100)
}

#[test]
fn anyhow_error_can_receive_framework_and_business_errors() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::PRE_UPDATE, fail_to_load);
    app.world().send_async_event(LoadRequest);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<Result<u32, anyhow::Error>>()
            .into_iter()
            .next()
        {
            assert_eq!(result.as_ref().unwrap_err().to_string(), "business failure");
            break;
        }
        assert!(Instant::now() < deadline, "async response timed out");
        std::thread::yield_now();
    }
}

#[test]
fn async_context_is_injected_into_a_named_handler() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::PRE_UPDATE, report_progress);
    app.world().send_async_event(ProgressRequest);
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
        50
    );
    assert!(matches!(
        app.world()
            .event_reader::<Result<u32, anyhow::Error>>()
            .into_iter()
            .next(),
        Some(Ok(100))
    ));
}
