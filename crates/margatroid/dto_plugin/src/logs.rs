use app_runtime_plugin::RuntimeEventSender;
use log_plugin::{TracingStream, TracingStreamError};
use margatroid_protocol::{IntoDto, LogRecordDto, ServerMessage};

use crate::{WebSocketMessageSend, WebSocketMessageTarget};

pub(crate) async fn forward_logs(stream: TracingStream, events: RuntimeEventSender) {
    let mut subscription = stream.subscribe();
    loop {
        let record = match subscription.recv().await {
            Ok(record) => record,
            Err(TracingStreamError::Lagged(count)) => {
                tracing::warn!(dropped = count, "DTO log stream lagged");
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
