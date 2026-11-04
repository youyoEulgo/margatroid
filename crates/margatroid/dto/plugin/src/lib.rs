mod handler;
mod system;
mod types;

pub use types::WebSocketMessageSend;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncContext, AsyncRuntimeHandle, WorldAsyncExt};
use axum_plugin::{AppAxumExt, AxumHandle};
use config_core::MargatroidConfig;
use core_plugin::{App, Plugin};
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{LogFieldDto, LogRecordDto};
use types::{
    BackendStateReportCache, ConnState, DtoAsyncError, DtoState, LogReady, PendingRequests,
};

#[derive(Default)]
pub struct DtoPlugin;
impl DtoPlugin {
    pub fn new() -> Self {
        Self
    }
}
impl Plugin for DtoPlugin {
    fn build(self, app: &mut App) {
        assert!(
            app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "DtoPlugin requires runtime and async facilities"
        );
        let handle = app
            .world()
            .get_resource::<AxumHandle>()
            .expect("DtoPlugin requires AxumPlugin");
        let lifetime = handle.lifetime();
        let config = app
            .world()
            .get_resource::<MargatroidConfig>()
            .expect("DtoPlugin requires configuration");
        let state = DtoState {
            ingress: config.server_ingress().clone(),
            events: std::sync::Arc::new(std::sync::OnceLock::new()),
            connections: ConnState::default(),
            lifetime: lifetime.clone(),
        };
        assert!(
            !app.world().contains_resource::<PendingRequests>(),
            "DtoPlugin is already installed"
        );
        // Initialize during closure dispatch, before the Axum startup task can listen.
        // Until dispatch the queued closure and Router own no event-queue sender.
        let events = state.events.clone();
        app.world()
            .send_async_closure(RuntimePlugin::UPDATE, move |context: AsyncContext| {
                let _ = events.set(context);
                async { Ok::<_, DtoAsyncError>(types::DtoInitialized) }
            });
        app.world_mut().insert_resource(state.clone());
        app.world_mut().insert_resource(state.connections.clone());
        app.world_mut().insert_resource(PendingRequests::default());
        app.world_mut()
            .insert_resource(types::TransportStatus::default());
        app.world_mut().insert_resource(BackendStateReportCache {
            dirty: true,
            ..Default::default()
        });
        if let Some(stream) = app.world().get_resource::<TracingStream>() {
            let mut subscription = stream.subscribe();
            app.world().send_async_closure(RuntimePlugin::UPDATE, move |events: AsyncContext| async move {
                loop {
                    let received = tokio::select! { biased; _ = lifetime.cancelled() => break, record = subscription.recv() => record };
                    match received {
                        Ok(record) => events.send_event(LogReady(LogRecordDto {
                            timestamp_millis: record.timestamp_millis, level: record.level, target: record.target, message: record.message,
                            fields: record.fields.into_iter().map(|field| LogFieldDto { name: field.name, value: field.value }).collect(), spans: record.spans,
                        })),
                        Err(TracingStreamError::Lagged(_)) => (),
                        Err(_) => break,
                    }
                }
                Ok::<_, DtoAsyncError>(())
            });
        }
        app.add_axum_routes(
            axum::Router::new()
                .route("/ws", axum::routing::get(handler::ws_handler))
                .with_state(state),
        )
        .add_system(RuntimePlugin::PRE_UPDATE, system::abort_control_system)
        .add_system(RuntimePlugin::UPDATE, system::dto_route_system)
        .add_system(
            RuntimePlugin::UPDATE,
            system::collect_external_events_system,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn app(with_logs: bool) -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(axum_plugin::AxumPlugin::bind(
                "127.0.0.1:0".parse().unwrap(),
            ));
        if with_logs {
            app.add_plugin(
                log_plugin::LogPlugin::default()
                    .without_console()
                    .with_stream(8),
            );
        }
        app.world_mut().insert_resource(
            MargatroidConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                config_core::LogLevel::Info,
                None,
                vec![config_core::OutboundTarget::Broadcast],
                vec![config_core::OutboundTarget::Broadcast],
            )
            .unwrap(),
        );
        app.add_plugin(DtoPlugin);
        app
    }

    #[test]
    fn dropping_before_dispatch_releases_router_context_and_log_subscription() {
        // Cover both an untouched App and a Router already queued by STARTUP.
        for queue_router in [false, true] {
            let mut app = app(true);
            let stream = app.world().get_resource::<TracingStream>().unwrap().clone();
            let events = Arc::downgrade(&app.world().get_resource::<DtoState>().unwrap().events);
            assert_eq!(stream.receiver_count(), 1);
            assert!(events.upgrade().unwrap().get().is_none());
            if queue_router {
                app.add_system(RuntimePlugin::STARTUP, |_world: &mut core_plugin::World| {
                    panic!("interrupt before queued closures are dispatched");
                });
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.tick())).is_err()
                );
            }
            drop(app);
            assert!(
                events.upgrade().is_none(),
                "queued Router retained DTO context after App drop"
            );
            assert_eq!(
                stream.receiver_count(),
                0,
                "queued log closure retained subscription after App drop"
            );
        }
    }

    #[test]
    fn event_context_is_ready_before_first_native_websocket_request() {
        let mut app = app(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = app.world().get_resource::<AxumHandle>().unwrap().clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        let address = loop {
            app.tick();
            if let Some(address) = handle.local_address() {
                break address;
            }
            assert!(Instant::now() < deadline, "Axum startup timed out");
            std::thread::yield_now();
        };
        assert!(
            app.world()
                .get_resource::<DtoState>()
                .unwrap()
                .events
                .get()
                .is_some()
        );
        let (mut socket, _) = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                tokio_tungstenite::connect_async(format!("ws://{address}/ws")),
            )
            .await
            .unwrap()
            .unwrap()
        });
        runtime
            .block_on(socket.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"type":"state.query","id":"first","message":{}}"#.into(),
            )))
            .unwrap();
        let response = loop {
            app.tick();
            if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) = runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_millis(10), socket.next()).await
                })
            {
                break text;
            }
            assert!(Instant::now() < deadline, "first DTO response timed out");
        };
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["type"], "request.failed");
        assert_eq!(response["id"], "first");
        drop(socket);
        drop(app);
    }
}
