use api_plugin::{WebSocketMessageSend, WebSocketMessageTarget};
use app_runtime_plugin::RuntimeEventSender;
use log_plugin::{TracingRecord, TracingStream, TracingStreamError};
use margatroid_protocol::{LogFieldDto, LogRecordDto, ServerEvent};

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
        events.send_event(WebSocketMessageSend {
            target: WebSocketMessageTarget::Broadcast,
            message: ServerEvent::Log {
                record: log_record(record),
            },
        });
    }
}

fn log_record(record: TracingRecord) -> LogRecordDto {
    LogRecordDto {
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
    }
}
