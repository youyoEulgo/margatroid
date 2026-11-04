use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use axum::{extract::ws::WebSocketUpgrade, routing::get, Extension, Router};
use axum_plugin::{AppAxumExt, AxumErrorKind, AxumHandle, AxumLifetime, AxumPlugin};
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
};

fn app(bind: SocketAddr) -> (App, AxumHandle) {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AxumPlugin::bind(bind));
    let handle = app.world().get_resource::<AxumHandle>().unwrap().clone();
    (app, handle)
}

async fn start(app: &mut App, handle: &AxumHandle) -> SocketAddr {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick();
            if let Some(address) = handle.local_address() {
                return address;
            }
            assert!(!handle.is_stopped());
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}

async fn stopped(handle: &AxumHandle) {
    tokio::time::timeout(Duration::from_secs(5), handle.wait_stopped())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn native_http_and_ecs_keep_running_without_request_events() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    app.add_axum_routes(Router::new().route(
        "/native",
        get(|| async { ([("x-native", "yes")], "native response") }),
    ));
    let ticks = Arc::new(AtomicUsize::new(0));
    let observed = ticks.clone();
    app.add_system(RuntimePlugin::UPDATE, move |_: &mut World| {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let address = start(&mut app, &handle).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /native HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("x-native: yes"));
    assert!(response.ends_with("native response"));
    let before = ticks.load(Ordering::Relaxed);
    app.tick();
    assert!(ticks.load(Ordering::Relaxed) > before);
    handle.request_shutdown();
    stopped(&handle).await;
    assert_eq!(handle.result(), Some(Ok(())));
}

#[tokio::test]
async fn upgraded_work_is_registered_before_upgrade_and_delays_completion() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    let entered = Arc::new(Notify::new());
    let cancelled = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let route_entered = entered.clone();
    let route_cancelled = cancelled.clone();
    let route_release = release.clone();
    app.add_axum_routes(Router::new().route(
        "/ws",
        get(
            move |upgrade: WebSocketUpgrade, Extension(lifetime): Extension<AxumLifetime>| {
                let guard = lifetime.track();
                let entered = route_entered.clone();
                let cancelled = route_cancelled.clone();
                let release = route_release.clone();
                async move {
                    upgrade.on_upgrade(move |socket| async move {
                        let _guard = guard;
                        entered.notify_one();
                        lifetime.cancelled().await;
                        cancelled.notify_one();
                        release.notified().await;
                        drop(socket);
                    })
                }
            },
        ),
    ));
    let address = start(&mut app, &handle).await;
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    handle.request_shutdown();
    tokio::time::timeout(Duration::from_secs(5), cancelled.notified())
        .await
        .unwrap();
    assert!(handle.lifetime().is_cancelled());
    assert!(!handle.is_stopped());
    release.notify_one();
    stopped(&handle).await;
    drop(socket);
}

#[tokio::test]
async fn bind_failure_completes_without_more_ecs_ticks() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (mut app, handle) = app(listener.local_addr().unwrap());
    app.tick();
    app.tick();
    let error = tokio::time::timeout(Duration::from_secs(5), handle.wait_stopped())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, AxumErrorKind::Bind);
    assert!(handle.is_stopped());
    assert!(handle.local_address().is_none());
}

#[tokio::test]
async fn routes_freeze_at_startup() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    start(&mut app, &handle).await;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.add_axum_routes(Router::new().route("/late", get(|| async { "late" })));
    }));
    assert!(outcome.is_err());
    handle.request_shutdown();
    stopped(&handle).await;
}

#[tokio::test]
async fn ordinary_request_finishes_before_shutdown_completion() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let route_entered = entered.clone();
    let route_release = release.clone();
    app.add_axum_routes(Router::new().route(
        "/slow",
        get(move || {
            let entered = route_entered.clone();
            let release = route_release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                "finished"
            }
        }),
    ));
    let address = start(&mut app, &handle).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    handle.request_shutdown();
    assert!(!handle.is_stopped());
    release.notify_one();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.ends_with("finished"));
    stopped(&handle).await;
}

#[test]
fn dropping_queued_startup_task_preserves_completion() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    app.tick();
    drop(app);
    assert!(handle.lifetime().is_cancelled());
    assert_eq!(
        handle.result().unwrap().unwrap_err().kind,
        AxumErrorKind::Cancelled
    );
}

#[tokio::test]
async fn app_drop_requests_shutdown_without_blocking_join_in_plugin() {
    let (mut app, handle) = app("127.0.0.1:0".parse().unwrap());
    start(&mut app, &handle).await;
    drop(app);
    assert!(handle.lifetime().is_cancelled());
    assert!(handle.is_stopped());
}
