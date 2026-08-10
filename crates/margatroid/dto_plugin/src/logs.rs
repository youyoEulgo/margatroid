use app_runtime_plugin::RuntimeEventSender;
use config_plugin::WebSocketMessageTarget;
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{IntoDto, LogRecordDto, ServerMessage};

use crate::WebSocketMessageSend;

pub(crate) async fn forward_logs(
    stream: TracingStream,
    events: RuntimeEventSender,
    targets: Vec<WebSocketMessageTarget>,
) {
    let mut subscription = stream.subscribe();
    loop {
        let record = match subscription.recv().await {
            Ok(record) => record,
            Err(TracingStreamError::Lagged(_)) => continue,
            Err(TracingStreamError::Closed) | Err(_) => break,
        };
        let Ok(record): Result<LogRecordDto, _> = record.into_dto(()) else {
            continue;
        };
        for target in &targets {
            events.send_event(WebSocketMessageSend {
                target: target.clone(),
                message: ServerMessage::Log {
                    record: record.clone(),
                },
            });
        }
    }
}
