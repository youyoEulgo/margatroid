use std::fmt;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogField {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    pub timestamp_millis: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: Vec<LogField>,
    pub spans: Vec<String>,
}

#[derive(Clone)]
pub struct LogStream {
    sender: broadcast::Sender<LogRecord>,
}

impl LogStream {
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> LogSubscription {
        LogSubscription {
            receiver: self.sender.subscribe(),
            dropped: 0,
        }
    }

    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    pub(crate) fn layer(&self) -> LogStreamLayer {
        LogStreamLayer {
            sender: self.sender.clone(),
        }
    }
}

pub struct LogSubscription {
    receiver: broadcast::Receiver<LogRecord>,
    dropped: u64,
}

impl LogSubscription {
    pub async fn recv(&mut self) -> Result<LogRecord, LogStreamError> {
        match self.receiver.recv().await {
            Ok(record) => Ok(record),
            Err(broadcast::error::RecvError::Closed) => Err(LogStreamError::Closed),
            Err(broadcast::error::RecvError::Lagged(count)) => {
                self.dropped = self.dropped.saturating_add(count);
                Err(LogStreamError::Lagged(count))
            }
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogStreamError {
    Closed,
    Lagged(u64),
}

impl fmt::Display for LogStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "log stream closed"),
            Self::Lagged(count) => write!(f, "log stream dropped {count} records"),
        }
    }
}

impl std::error::Error for LogStreamError {}

pub(crate) struct LogStreamLayer {
    sender: broadcast::Sender<LogRecord>,
}

impl<S> Layer<S> for LogStreamLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let record = record_from_event(event, context);
        let _ = self.sender.send(record);
    }
}

pub(crate) struct JsonLayer<W> {
    writer: W,
}

impl<W> JsonLayer<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<S, W> Layer<S> for JsonLayer<W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let record = record_from_event(event, context);
        let Ok(mut encoded) = serde_json::to_vec(&record) else {
            return;
        };
        encoded.push(b'\n');
        let mut writer = self.writer.make_writer_for(event.metadata());
        let _ = writer.write_all(&encoded);
    }
}

fn record_from_event<S>(event: &Event<'_>, context: Context<'_, S>) -> LogRecord
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let metadata = event.metadata();
    let mut visitor = FieldVisitor::default();
    event.record(&mut visitor);
    let message = visitor
        .fields
        .iter()
        .find(|field| field.name == "message")
        .map(|field| field.value.clone())
        .unwrap_or_default();
    let spans = context
        .event_scope(event)
        .map(|scope| {
            scope
                .from_root()
                .map(|span| span.metadata().name().to_string())
                .collect()
        })
        .unwrap_or_default();
    LogRecord {
        timestamp_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        level: metadata.level().to_string(),
        target: metadata.target().to_string(),
        message,
        fields: visitor.fields,
        spans,
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: Vec<LogField>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields.push(LogField {
            name: field.name().to_string(),
            value: format!("{value:?}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::prelude::*;

    use super::*;

    #[tokio::test]
    async fn stream_captures_structured_event() {
        let stream = LogStream::new(8);
        let mut subscription = stream.subscribe();
        let subscriber = tracing_subscriber::registry().with(stream.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(request_id = 7, "request completed");
        });

        let record = subscription.recv().await.unwrap();
        assert_eq!(record.level, "INFO");
        assert_eq!(record.message, "request completed");
        assert!(record
            .fields
            .iter()
            .any(|field| field.name == "request_id" && field.value == "7"));
    }

    #[tokio::test]
    async fn subscription_reports_lag() {
        let stream = LogStream::new(1);
        let mut subscription = stream.subscribe();
        let layer = stream.layer();
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(sequence = 1, "first");
            tracing::info!(sequence = 2, "second");
        });

        assert_eq!(subscription.recv().await, Err(LogStreamError::Lagged(1)));
        assert_eq!(subscription.dropped_count(), 1);
        assert_eq!(subscription.recv().await.unwrap().message, "second");
    }
}
