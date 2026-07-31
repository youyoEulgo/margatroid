use std::time::{Duration, Instant};

use anyhow::anyhow;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AppAsyncExt, AsyncRequest, AsyncRuntimePlugin};
use core_plugin::App;

#[test]
fn anyhow_error_can_receive_framework_and_business_errors() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .register_async_request::<u32, anyhow::Error>();
    app.world()
        .event_write()
        .send_event(AsyncRequest::<u32, anyhow::Error>::new(|| async {
            Err(anyhow!("business failure"))
        }));

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
