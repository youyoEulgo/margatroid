mod handler;
mod system;
mod types;

pub use types::WebSocketMessageSend;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::{AsyncRuntimeHandle, WorldAsyncExt};
use config_core::MargatroidConfig;
use core_plugin::{App, Plugin};
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{LogFieldDto, LogRecordDto};
use server_plugin::{AppServerExt, WebSocketConnections};
use types::{BackendStateReportCache, LogReady, PendingRequests};

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
        assert!(
            app.world().contains_resource::<WebSocketConnections>(),
            "DtoPlugin requires ServerPlugin"
        );
        assert!(
            app.world().contains_resource::<MargatroidConfig>(),
            "DtoPlugin requires outbound configuration"
        );
        assert!(
            !app.world().contains_resource::<PendingRequests>(),
            "DtoPlugin is already installed"
        );
        app.world_mut().insert_resource(PendingRequests::default());
        app.world_mut().insert_resource(BackendStateReportCache {
            dirty: true,
            ..Default::default()
        });
        if let Some(stream) = app.world().get_resource::<TracingStream>() {
            let mut subscription = stream.subscribe();
            let events = app_runtime_plugin::WorldEventExt::event_sender(app.world());
            app.world().spawn_async_service(async move {
                loop {
                    match subscription.recv().await {
                        Ok(record) => events.send_event(LogReady(LogRecordDto {
                            timestamp_millis: record.timestamp_millis,
                            level: record.level,
                            target: record.target,
                            message: record.message,
                            fields: record
                                .fields
                                .into_iter()
                                .map(|field| LogFieldDto {
                                    name: field.name,
                                    value: field.value,
                                })
                                .collect(),
                            spans: record.spans,
                        })),
                        Err(TracingStreamError::Lagged(_)) => (),
                        Err(_) => break,
                    }
                }
            });
        }
        app.add_websocket_event_route("/ws")
            .add_system(RuntimePlugin::PRE_UPDATE, system::abort_control_system)
            .add_system(RuntimePlugin::UPDATE, system::dto_route_system)
            .add_system(
                RuntimePlugin::UPDATE,
                system::collect_external_events_system,
            );
    }
}
