use app_runtime_plugin::{AppControl, AppShutdownExt};
use async_runtime_plugin::AsyncRuntimeStatus;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use core_plugin::{App, Plugin, Stage, World};
use http_server_plugin::{HttpAppExt, HttpServerHandle};
use signal_plugin::{ProcessSignal, ProcessSignalReceived, SignalListenerFailed};

use crate::{DaemonLifecycle, DaemonState};

#[derive(Clone, Copy, Debug, Default)]
pub struct DaemonLifecyclePlugin;

impl Plugin for DaemonLifecyclePlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().resource::<AppControl>().is_some(),
            "AppRuntimePlugin must be installed before DaemonLifecyclePlugin"
        );
        assert!(
            app.world().resource::<AsyncRuntimeStatus>().is_some(),
            "AsyncRuntimePlugin must be installed before DaemonLifecyclePlugin"
        );
        assert!(
            app.world().resource::<HttpServerHandle>().is_some(),
            "HttpServerPlugin must be installed before DaemonLifecyclePlugin"
        );
        assert!(
            app.world().resource::<DaemonLifecycle>().is_none(),
            "DaemonLifecyclePlugin can only be installed once"
        );

        let lifecycle = DaemonLifecycle::default();
        app.add_http_routes(
            Router::new()
                .route("/ready", get(readiness))
                .with_state(lifecycle.clone()),
        );
        app.add_resource(lifecycle.clone());

        app.add_event::<ProcessSignalReceived>();
        app.add_event::<SignalListenerFailed>();
        let mut signal_reader = app.event_reader::<ProcessSignalReceived>();
        let mut signal_failure_reader = app.event_reader::<SignalListenerFailed>();
        let mut readiness_signal_failure_reader = app.event_reader::<SignalListenerFailed>();
        let signal_control = app.world().resource::<AppControl>().unwrap().clone();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                let failures = world.read_events(&mut signal_failure_reader);
                for failure in &failures {
                    tracing::error!(message = %failure.message, "signal listener failed");
                }
                let should_shutdown =
                    world
                        .read_events(&mut signal_reader)
                        .into_iter()
                        .any(|event| {
                            matches!(
                                event.signal,
                                ProcessSignal::Interrupt | ProcessSignal::Terminate
                            )
                        });
                if should_shutdown || !failures.is_empty() {
                    signal_control.shutdown();
                }
            }],
        );

        let startup_lifecycle = lifecycle.clone();
        app.add_systems(
            Stage::First,
            [move |world: &mut World| {
                if startup_lifecycle.state() != DaemonState::Starting {
                    return;
                }
                let control = world
                    .resource::<AppControl>()
                    .expect("AppControl should be registered");
                if control.is_shutdown() {
                    return;
                }
                let http_ready = world
                    .resource::<HttpServerHandle>()
                    .is_some_and(|server| server.address().is_some());
                let async_ready = world
                    .resource::<AsyncRuntimeStatus>()
                    .is_some_and(AsyncRuntimeStatus::is_running);
                let signal_ready = world
                    .read_events(&mut readiness_signal_failure_reader)
                    .is_empty();
                if http_ready && async_ready && signal_ready {
                    startup_lifecycle.set(DaemonState::Ready);
                    tracing::info!("margatroidd ready");
                } else {
                    tracing::error!(
                        http_ready,
                        async_ready,
                        signal_ready,
                        "daemon dependencies failed to start"
                    );
                    control.shutdown();
                }
            }],
        );

        let stopped_lifecycle = lifecycle.clone();
        app.after_shutdown(move |_world| {
            stopped_lifecycle.set(DaemonState::Stopped);
            tracing::info!("margatroidd stopped");
        });
        app.on_shutdown(move |_world| {
            lifecycle.set(DaemonState::Draining);
            tracing::info!("margatroidd draining");
        });
    }
}

async fn readiness(State(lifecycle): State<DaemonLifecycle>) -> (StatusCode, &'static str) {
    if lifecycle.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use app_runtime_plugin::{AppRunExt, AppRuntimePlugin};
    use async_runtime_plugin::AsyncRuntimePlugin;
    use http_server_plugin::HttpServerPlugin;

    use super::*;

    #[test]
    fn becomes_ready_and_performs_ordered_shutdown() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_plugins(
            HttpServerPlugin::bind("127.0.0.1:0").with_shutdown_timeout(Duration::from_millis(100)),
        );
        app.add_plugins(DaemonLifecyclePlugin);
        let control = app.world().resource::<AppControl>().unwrap().clone();
        let lifecycle = app.world().resource::<DaemonLifecycle>().unwrap().clone();
        let server = app.world().resource::<HttpServerHandle>().unwrap().clone();
        let runtime = app
            .world()
            .resource::<AsyncRuntimeStatus>()
            .unwrap()
            .clone();
        let thread = std::thread::spawn(move || app.run());

        let deadline = Instant::now() + Duration::from_secs(2);
        while lifecycle.state() != DaemonState::Ready {
            assert!(
                Instant::now() < deadline,
                "daemon readiness timed out: lifecycle={:?}, server={:?}, async_running={}",
                lifecycle.state(),
                server.address(),
                runtime.is_running()
            );
            std::thread::yield_now();
        }
        let address = server.address().unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(b"GET /ready HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

        control.shutdown();
        thread.join().unwrap();

        assert_eq!(lifecycle.state(), DaemonState::Stopped);
        assert!(!server.is_running());
        assert!(!runtime.is_running());
    }

    #[test]
    fn signal_listener_failure_never_exposes_ready() {
        let mut app = App::new();
        app.add_plugins(AppRuntimePlugin);
        app.add_plugins(AsyncRuntimePlugin::default());
        app.add_plugins(
            HttpServerPlugin::bind("127.0.0.1:0").with_shutdown_timeout(Duration::from_millis(100)),
        );
        app.add_plugins(DaemonLifecyclePlugin);
        app.world().send_event(SignalListenerFailed {
            message: "test failure".into(),
        });

        let lifecycle = app.world().resource::<DaemonLifecycle>().unwrap().clone();
        let exposed_ready = Arc::new(AtomicBool::new(false));
        let observed_ready = exposed_ready.clone();
        app.add_systems(
            Stage::First,
            [move |_world: &mut World| {
                observed_ready.store(lifecycle.is_ready(), Ordering::Release);
            }],
        );

        app.run();

        assert!(!exposed_ready.load(Ordering::Acquire));
        assert_eq!(
            app.world().resource::<DaemonLifecycle>().unwrap().state(),
            DaemonState::Stopped
        );
    }
}
