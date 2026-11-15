use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{SystemTime, UNIX_EPOCH};

use core_plugin::Resource;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingField {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingRecord {
    pub timestamp_millis: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: Vec<TracingField>,
    pub spans: Vec<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub module: Option<String>,
}

#[derive(Clone)]
pub struct TracingStream {
    sender: broadcast::Sender<TracingRecord>,
    sinks: Arc<Mutex<BTreeMap<u64, DurableSink>>>,
}
type DurableSink = Arc<dyn Fn(&TracingRecord) + Send + Sync>;
pub struct TracingSinkGuard {
    sinks: Weak<Mutex<BTreeMap<u64, DurableSink>>>,
    id: u64,
}
impl Resource for TracingSinkGuard {}
impl Drop for TracingSinkGuard {
    fn drop(&mut self) {
        if let Some(sinks) = self.sinks.upgrade() {
            sinks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.id);
        }
    }
}

impl TracingStream {
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            sinks: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn layer(&self) -> TracingStreamLayer {
        TracingStreamLayer {
            sender: self.sender.clone(),
            sinks: self.sinks.clone(),
        }
    }

    /// Durable observers run before the best-effort broadcast, without a routing queue.
    pub fn subscribe_persistent(
        &self,
        sink: impl Fn(&TracingRecord) + Send + Sync + 'static,
    ) -> TracingSinkGuard {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.sinks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, Arc::new(sink));
        TracingSinkGuard {
            sinks: Arc::downgrade(&self.sinks),
            id,
        }
    }
    pub fn subscribe(&self) -> TracingSubscription {
        TracingSubscription {
            receiver: self.sender.subscribe(),
            dropped: 0,
        }
    }

    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Resource for TracingStream {}

pub struct TracingSubscription {
    receiver: broadcast::Receiver<TracingRecord>,
    dropped: u64,
}

impl TracingSubscription {
    pub async fn recv(&mut self) -> Result<TracingRecord, TracingStreamError> {
        match self.receiver.recv().await {
            Ok(record) => Ok(record),
            Err(broadcast::error::RecvError::Closed) => Err(TracingStreamError::Closed),
            Err(broadcast::error::RecvError::Lagged(count)) => {
                self.dropped = self.dropped.saturating_add(count);
                Err(TracingStreamError::Lagged(count))
            }
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }
}

#[non_exhaustive]
#[derive(Debug)]
pub enum TracingStreamError {
    Closed,
    Lagged(u64),
}

impl fmt::Display for TracingStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("tracing stream closed"),
            Self::Lagged(count) => write!(formatter, "tracing stream dropped {count} records"),
        }
    }
}

impl std::error::Error for TracingStreamError {}

pub(crate) struct TracingStreamLayer {
    sender: broadcast::Sender<TracingRecord>,
    sinks: Arc<Mutex<BTreeMap<u64, DurableSink>>>,
}

impl<S> Layer<S> for TracingStreamLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let record = record_from_event(event, context);
        let sinks = self
            .sinks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for sink in sinks {
            sink(&record);
        }
        let _ = self.sender.send(record);
    }
}

pub(crate) struct JsonLayer<Writer> {
    writer: Writer,
}

impl<Writer> JsonLayer<Writer> {
    pub(crate) fn new(writer: Writer) -> Self {
        Self { writer }
    }
}

impl<S, Writer> Layer<S> for JsonLayer<Writer>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    Writer: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
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

fn record_from_event<S>(event: &Event<'_>, context: Context<'_, S>) -> TracingRecord
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
    TracingRecord {
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
        file: metadata.file().map(str::to_owned),
        line: metadata.line(),
        module: metadata.module_path().map(str::to_owned),
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: Vec<TracingField>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields.push(TracingField {
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
        let stream = TracingStream::new(8);
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
        let stream = TracingStream::new(1);
        let mut subscription = stream.subscribe();
        let subscriber = tracing_subscriber::registry().with(stream.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(sequence = 1, "first");
            tracing::info!(sequence = 2, "second");
        });

        assert!(matches!(
            subscription.recv().await,
            Err(TracingStreamError::Lagged(1))
        ));
        assert_eq!(subscription.dropped_count(), 1);
        assert_eq!(subscription.recv().await.unwrap().message, "second");
    }
    #[tokio::test]
    async fn persistent_observer_receives_every_record_before_broadcast_can_lag_and_unlinks_on_drop(
    ) {
        let stream = TracingStream::new(1);
        let mut subscription = stream.subscribe();
        let saved = Arc::new(Mutex::new(Vec::new()));
        let records = saved.clone();
        let guard =
            stream.subscribe_persistent(move |record| records.lock().unwrap().push(record.clone()));
        let subscriber = tracing_subscriber::registry().with(stream.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(request_id = "one", "first durable");
            tracing::warn!(request_id = "two", "second durable");
        });
        assert_eq!(
            saved
                .lock()
                .unwrap()
                .iter()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            vec!["first durable", "second durable"]
        );
        assert!(saved
            .lock()
            .unwrap()
            .iter()
            .all(|record| record.file.is_some()
                && record.line.is_some()
                && record.module.is_some()));
        assert!(matches!(
            subscription.recv().await,
            Err(TracingStreamError::Lagged(1))
        ));
        drop(guard);
        let subscriber = tracing_subscriber::registry().with(stream.layer());
        tracing::subscriber::with_default(subscriber, || tracing::info!("after unlink"));
        assert_eq!(saved.lock().unwrap().len(), 2);
    }
}
