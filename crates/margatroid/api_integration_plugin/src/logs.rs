use app_runtime_plugin::RuntimeEventSender;
use core_plugin::World;
use dto_plugin::{WebSocketMessageSend, WebSocketMessageTarget};
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{IntoDto, LogRecordDto, ServerMessage};
use server_plugin::{ServerFailed, ServerStarted, ServerStopped};

pub(crate) fn report_server_events(world: &mut World) {
    for event in world.event_reader::<ServerStarted>() {
        tracing::info!(address = %event.address, "daemon WebSocket server started");
    }
    for event in world.event_reader::<ServerFailed>() {
        tracing::error!(error = %event.message, "daemon WebSocket server failed");
    }
    if !world.event_reader::<ServerStopped>().is_empty() {
        tracing::info!("daemon WebSocket server stopped");
    }
}

pub(crate) async fn forward_logs(stream: TracingStream, events: RuntimeEventSender) {
    let mut subscription = stream.subscribe();
    loop {
        let record = match subscription.recv().await {
            Ok(record) => record,
            Err(TracingStreamError::Lagged(count)) => {
                tracing::warn!(dropped = count, "API log stream lagged");
                continue;
            }
            Err(TracingStreamError::Closed) | Err(_) => break,
        };
        let Ok(record): Result<LogRecordDto, _> = record.into_dto(()) else {
            continue;
        };
        events.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerMessage::Log { record },
        });
    }
}
