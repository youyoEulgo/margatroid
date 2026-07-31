use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use app_runtime_plugin::{RuntimePlugin, POST_UPDATE};
use core_plugin::App;
use log_plugin::{
    EventLog, LogLevel, LogPlugin, TracingStream, WorldEventLogExt, EVENT_LOG_TARGET,
};

const STREAM_CAPACITY: usize = 8;

fn install_runtime_and_log(app: &mut App) {
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(LogPlugin::default().with_stream(STREAM_CAPACITY));
}

#[test]
fn concurrent_apps_reuse_the_same_process_configuration() {
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            let mut app = App::new();
            app.add_plugin(RuntimePlugin::default());
            barrier.wait();
            app.add_plugin(LogPlugin::default().with_stream(STREAM_CAPACITY));
            app.world().contains_resource::<TracingStream>()
        }));
    }

    assert!(threads.into_iter().all(|thread| thread.join().unwrap()));
}

#[test]
fn app_schedule_does_not_participate_in_process_configuration() {
    let mut app = App::new();
    app.add_schedule("logs".into()).add_plugin(
        LogPlugin::default()
            .with_stream(STREAM_CAPACITY)
            .in_schedule("logs"),
    );

    assert!(app.world().contains_resource::<TracingStream>());
}

#[test]
#[should_panic(expected = "schedule `missing` does not exist")]
fn missing_schedule_is_rejected_before_installation() {
    let mut app = App::new();
    app.add_plugin(
        LogPlugin::default()
            .with_stream(STREAM_CAPACITY)
            .in_schedule("missing"),
    );
}

#[test]
#[should_panic(expected = "LogPlugin is already installed in this App")]
fn plugin_cannot_be_installed_twice_in_one_app() {
    let mut app = App::new();
    install_runtime_and_log(&mut app);
    app.add_plugin(LogPlugin::default().with_stream(STREAM_CAPACITY));
}

#[tokio::test]
async fn event_log_is_projected_to_tracing_without_exclusive_consumption() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reader_seen = Arc::clone(&seen);
    let mut app = App::new();
    install_runtime_and_log(&mut app);
    app.add_system(POST_UPDATE, move |world: &mut core_plugin::World| {
        reader_seen.lock().unwrap().extend(
            world
                .event_reader::<EventLog>()
                .into_iter()
                .map(|event| event.message.clone()),
        );
    });

    let stream = app.world().get_resource::<TracingStream>().unwrap().clone();
    let mut subscription = stream.subscribe();
    app.world().event_log(LogLevel::Info, "workspace started");
    app.tick();

    let record = tokio::time::timeout(Duration::from_secs(1), subscription.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.target, EVENT_LOG_TARGET);
    assert_eq!(record.message, "workspace started");
    assert_eq!(*seen.lock().unwrap(), ["workspace started"]);
}
