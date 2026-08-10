use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::ws::{CloseFrame, Message};
use core_plugin::{Event, Resource};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{WebSocketNameError, WebSocketSendError, WebSocketStreamReceiveError};

pub type WebSocketMessage = Message;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WebSocketConnectionId(u64);

impl WebSocketConnectionId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterConnection {
    pub id: String,
    pub connection_id: WebSocketConnectionId,
    pub client_type: String,
}

impl Event for RegisterConnection {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WebSocketStreamId(String);

impl WebSocketStreamId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketStreamPhase {
    Start,
    Chunk,
    End,
    Abort,
}

pub enum WebSocketMessageClassification {
    Ordinary {
        message: WebSocketMessage,
    },
    Stream {
        stream_id: WebSocketStreamId,
        phase: WebSocketStreamPhase,
        message: WebSocketMessage,
    },
}

#[non_exhaustive]
#[derive(Debug)]
pub enum WebSocketProtocolError {
    InvalidEnvelope {
        message: String,
    },
    DuplicateStream {
        connection_id: WebSocketConnectionId,
        stream_id: WebSocketStreamId,
    },
    UnknownStream {
        connection_id: WebSocketConnectionId,
        stream_id: WebSocketStreamId,
    },
}

impl fmt::Display for WebSocketProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope { message } => {
                write!(formatter, "invalid WebSocket stream envelope: {message}")
            }
            Self::DuplicateStream {
                connection_id,
                stream_id,
            } => write!(
                formatter,
                "WebSocket stream `{}` already exists on connection {}",
                stream_id.as_str(),
                connection_id.get()
            ),
            Self::UnknownStream {
                connection_id,
                stream_id,
            } => write!(
                formatter,
                "WebSocket stream `{}` does not exist on connection {}",
                stream_id.as_str(),
                connection_id.get()
            ),
        }
    }
}

impl std::error::Error for WebSocketProtocolError {}

pub trait WebSocketMessageClassifier: Send + Sync + 'static {
    fn classify(
        &self,
        message: WebSocketMessage,
    ) -> Result<WebSocketMessageClassification, WebSocketProtocolError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JsonWebSocketMessageClassifier;

impl WebSocketMessageClassifier for JsonWebSocketMessageClassifier {
    fn classify(
        &self,
        message: WebSocketMessage,
    ) -> Result<WebSocketMessageClassification, WebSocketProtocolError> {
        let Message::Text(text) = &message else {
            return Ok(WebSocketMessageClassification::Ordinary { message });
        };
        let value: Value = match serde_json::from_str(text.as_str()) {
            Ok(value) => value,
            Err(_) => return Ok(WebSocketMessageClassification::Ordinary { message }),
        };
        let Some(envelope) = value.get("mecs_stream") else {
            return Ok(WebSocketMessageClassification::Ordinary { message });
        };
        let Some(envelope) = envelope.as_object() else {
            return Err(WebSocketProtocolError::InvalidEnvelope {
                message: "`mecs_stream` must be an object".into(),
            });
        };
        let stream_id = envelope
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| WebSocketProtocolError::InvalidEnvelope {
                message: "`mecs_stream.id` must be a non-empty string".into(),
            })?;
        let phase = match envelope.get("phase").and_then(Value::as_str) {
            Some("start") => WebSocketStreamPhase::Start,
            Some("chunk") => WebSocketStreamPhase::Chunk,
            Some("end") => WebSocketStreamPhase::End,
            Some("abort") => WebSocketStreamPhase::Abort,
            _ => {
                return Err(WebSocketProtocolError::InvalidEnvelope {
                    message: "`mecs_stream.phase` must be start, chunk, end, or abort".into(),
                });
            }
        };
        Ok(WebSocketMessageClassification::Stream {
            stream_id: WebSocketStreamId::new(stream_id),
            phase,
            message,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSocketCloseReason {
    code: u16,
    reason: String,
}

impl WebSocketCloseReason {
    pub fn new(code: u16, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> u16 {
        self.code
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) fn from_frame(frame: CloseFrame) -> Self {
        Self::new(frame.code, frame.reason.to_string())
    }

    fn into_frame(self) -> CloseFrame {
        CloseFrame {
            code: self.code,
            reason: self.reason.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    Open = 0,
    Closing = 1,
    Closed = 2,
}

pub(crate) struct SharedConnectionState(AtomicU8);

impl SharedConnectionState {
    pub(crate) fn open() -> Self {
        Self(AtomicU8::new(ConnectionState::Open as u8))
    }

    pub(crate) fn load(&self) -> ConnectionState {
        match self.0.load(Ordering::Acquire) {
            0 => ConnectionState::Open,
            1 => ConnectionState::Closing,
            _ => ConnectionState::Closed,
        }
    }

    fn begin_close(&self) -> bool {
        self.0
            .compare_exchange(
                ConnectionState::Open as u8,
                ConnectionState::Closing as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn close(&self) {
        self.0
            .store(ConnectionState::Closed as u8, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct WebSocketSender {
    connection_id: WebSocketConnectionId,
    sender: mpsc::Sender<WebSocketMessage>,
    state: Arc<SharedConnectionState>,
    send_lock: Arc<tokio::sync::Mutex<()>>,
}

impl WebSocketSender {
    pub(crate) fn new(
        connection_id: WebSocketConnectionId,
        sender: mpsc::Sender<WebSocketMessage>,
        state: Arc<SharedConnectionState>,
    ) -> Self {
        Self {
            connection_id,
            sender,
            state,
            send_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn connection_id(&self) -> WebSocketConnectionId {
        self.connection_id
    }

    pub async fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketSendError> {
        let _send = self.send_lock.lock().await;
        if self.state.load() != ConnectionState::Open {
            return Err(WebSocketSendError::ConnectionClosed);
        }
        self.sender
            .send(message)
            .await
            .map_err(|_| WebSocketSendError::ConnectionClosed)
    }

    pub fn try_send(&self, message: WebSocketMessage) -> Result<(), WebSocketSendError> {
        let _send = self
            .send_lock
            .try_lock()
            .map_err(|_| WebSocketSendError::BufferFull)?;
        if self.state.load() != ConnectionState::Open {
            return Err(WebSocketSendError::ConnectionClosed);
        }
        self.sender.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => WebSocketSendError::BufferFull,
            mpsc::error::TrySendError::Closed(_) => WebSocketSendError::ConnectionClosed,
        })
    }

    pub async fn close(&self, reason: WebSocketCloseReason) -> Result<(), WebSocketSendError> {
        let _send = self.send_lock.lock().await;
        if !self.state.begin_close() {
            return Err(WebSocketSendError::ConnectionClosed);
        }
        if self
            .sender
            .send(Message::Close(Some(reason.into_frame())))
            .await
            .is_err()
        {
            self.state.close();
            return Err(WebSocketSendError::ConnectionClosed);
        }
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.state.load() != ConnectionState::Open || self.sender.is_closed()
    }
}

pub struct WebSocketMessageSender {
    senders: Vec<WebSocketSender>,
    message: WebSocketMessage,
}

impl WebSocketMessageSender {
    pub fn new(senders: Vec<WebSocketSender>, message: WebSocketMessage) -> Self {
        Self { senders, message }
    }

    pub async fn send(self) -> Vec<(WebSocketConnectionId, Result<(), WebSocketSendError>)> {
        let mut results = Vec::with_capacity(self.senders.len());
        for sender in self.senders {
            let connection_id = sender.connection_id();
            results.push((connection_id, sender.send(self.message.clone()).await));
        }
        results
    }

    pub fn try_send(self) -> Vec<(WebSocketConnectionId, Result<(), WebSocketSendError>)> {
        self.senders
            .into_iter()
            .map(|sender| {
                let connection_id = sender.connection_id();
                (connection_id, sender.try_send(self.message.clone()))
            })
            .collect()
    }
}

struct WebSocketConnectionEntry {
    name: String,
    connection_type: String,
    sender: WebSocketSender,
}

#[derive(Default)]
struct WebSocketConnectionsState {
    by_id: HashMap<WebSocketConnectionId, WebSocketConnectionEntry>,
    by_name: HashMap<String, WebSocketConnectionId>,
    by_type: HashMap<String, HashSet<WebSocketConnectionId>>,
}

#[derive(Clone)]
pub struct WebSocketConnections {
    state: Arc<RwLock<WebSocketConnectionsState>>,
}

impl WebSocketConnections {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(WebSocketConnectionsState::default())),
        }
    }

    pub fn get(&self, connection_id: WebSocketConnectionId) -> Option<WebSocketSender> {
        self.state
            .read()
            .expect("WebSocket connection registry lock poisoned")
            .by_id
            .get(&connection_id)
            .map(|entry| entry.sender.clone())
    }

    pub fn get_by_name(&self, name: &str) -> Option<WebSocketSender> {
        if name.is_empty() {
            return None;
        }
        let state = self
            .state
            .read()
            .expect("WebSocket connection registry lock poisoned");
        let connection_id = state.by_name.get(name)?;
        state
            .by_id
            .get(connection_id)
            .map(|entry| entry.sender.clone())
    }

    pub fn name(&self, connection_id: WebSocketConnectionId) -> Option<String> {
        self.state
            .read()
            .expect("WebSocket connection registry lock poisoned")
            .by_id
            .get(&connection_id)
            .and_then(|entry| (!entry.name.is_empty()).then(|| entry.name.clone()))
    }

    pub fn connection_type(&self, connection_id: WebSocketConnectionId) -> Option<String> {
        self.state
            .read()
            .expect("WebSocket connection registry lock poisoned")
            .by_id
            .get(&connection_id)
            .and_then(|entry| {
                (!entry.connection_type.is_empty()).then(|| entry.connection_type.clone())
            })
    }

    pub fn set_connection_type(
        &self,
        connection_id: WebSocketConnectionId,
        connection_type: impl Into<String>,
    ) -> bool {
        let connection_type = connection_type.into();
        let mut state = self
            .state
            .write()
            .expect("WebSocket connection registry lock poisoned");
        let old_type = match state.by_id.get(&connection_id) {
            Some(entry) => entry.connection_type.clone(),
            None => return false,
        };
        if old_type == connection_type {
            return true;
        }
        if !old_type.is_empty() {
            remove_type_index(&mut state, &old_type, connection_id);
        }
        state
            .by_id
            .get_mut(&connection_id)
            .expect("WebSocket connection disappeared while registry was write locked")
            .connection_type = connection_type.clone();
        if !connection_type.is_empty() {
            state
                .by_type
                .entry(connection_type)
                .or_default()
                .insert(connection_id);
        }
        true
    }

    pub fn set_name(
        &self,
        connection_id: WebSocketConnectionId,
        name: impl Into<String>,
    ) -> Result<(), WebSocketNameError> {
        let name = name.into();
        let mut state = self
            .state
            .write()
            .expect("WebSocket connection registry lock poisoned");
        let old_name = state
            .by_id
            .get(&connection_id)
            .ok_or(WebSocketNameError::ConnectionNotFound { connection_id })?
            .name
            .clone();
        if old_name == name {
            return Ok(());
        }
        if let Some(existing_id) = state.by_name.get(&name) {
            if *existing_id != connection_id {
                return Err(WebSocketNameError::NameAlreadyExists { name });
            }
        }

        if !old_name.is_empty() {
            state.by_name.remove(&old_name);
        }
        state
            .by_id
            .get_mut(&connection_id)
            .expect("WebSocket connection disappeared while registry was write locked")
            .name
            .clone_from(&name);
        if !name.is_empty() {
            state.by_name.insert(name, connection_id);
        }
        Ok(())
    }

    pub fn get_all(&self) -> Vec<WebSocketSender> {
        self.state
            .read()
            .expect("WebSocket connection registry lock poisoned")
            .by_id
            .values()
            .map(|entry| entry.sender.clone())
            .collect()
    }

    pub fn get_by_type(&self, connection_type: &str) -> Vec<WebSocketSender> {
        if connection_type.is_empty() {
            return Vec::new();
        }
        let state = self
            .state
            .read()
            .expect("WebSocket connection registry lock poisoned");
        state
            .by_type
            .get(connection_type)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| state.by_id.get(id).map(|entry| entry.sender.clone()))
            .collect()
    }

    pub fn get_unnamed(&self) -> Vec<WebSocketSender> {
        self.state
            .read()
            .expect("WebSocket connection registry lock poisoned")
            .by_id
            .values()
            .filter(|entry| entry.name.is_empty())
            .map(|entry| entry.sender.clone())
            .collect()
    }

    pub(crate) fn insert(&self, sender: WebSocketSender) {
        let connection_id = sender.connection_id();
        let mut state = self
            .state
            .write()
            .expect("WebSocket connection registry lock poisoned");
        assert!(
            !state.by_id.contains_key(&connection_id),
            "WebSocket connection ID {} is already registered",
            connection_id.get()
        );
        state.by_id.insert(
            connection_id,
            WebSocketConnectionEntry {
                name: String::new(),
                connection_type: String::new(),
                sender,
            },
        );
    }

    pub(crate) fn remove(&self, connection_id: WebSocketConnectionId) {
        let mut state = self
            .state
            .write()
            .expect("WebSocket connection registry lock poisoned");
        let Some(entry) = state.by_id.remove(&connection_id) else {
            return;
        };
        if !entry.name.is_empty() {
            state.by_name.remove(&entry.name);
        }
        if !entry.connection_type.is_empty() {
            remove_type_index(&mut state, &entry.connection_type, connection_id);
        }
    }
}

fn remove_type_index(
    state: &mut WebSocketConnectionsState,
    connection_type: &str,
    connection_id: WebSocketConnectionId,
) {
    let remove_type = state
        .by_type
        .get_mut(connection_type)
        .map(|ids| {
            ids.remove(&connection_id);
            ids.is_empty()
        })
        .unwrap_or(false);
    if remove_type {
        state.by_type.remove(connection_type);
    }
}

impl Resource for WebSocketConnections {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamState {
    Open = 0,
    Finished = 1,
    Aborted = 2,
    ConnectionClosed = 3,
}

pub(crate) struct SharedStreamState(AtomicU8);

impl SharedStreamState {
    pub(crate) fn open() -> Self {
        Self(AtomicU8::new(StreamState::Open as u8))
    }

    pub(crate) fn load(&self) -> StreamState {
        match self.0.load(Ordering::Acquire) {
            0 => StreamState::Open,
            1 => StreamState::Finished,
            2 => StreamState::Aborted,
            _ => StreamState::ConnectionClosed,
        }
    }

    pub(crate) fn finish(&self) {
        self.0.store(StreamState::Finished as u8, Ordering::Release);
    }

    pub(crate) fn abort(&self) {
        self.0.store(StreamState::Aborted as u8, Ordering::Release);
    }

    pub(crate) fn disconnect(&self) {
        let _ = self.0.compare_exchange(
            StreamState::Open as u8,
            StreamState::ConnectionClosed as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

pub struct WebSocketStreamReceiver {
    receiver: mpsc::Receiver<WebSocketMessage>,
    state: Arc<SharedStreamState>,
    terminal_reported: bool,
}

impl WebSocketStreamReceiver {
    pub(crate) fn new(
        receiver: mpsc::Receiver<WebSocketMessage>,
        state: Arc<SharedStreamState>,
    ) -> Self {
        Self {
            receiver,
            state,
            terminal_reported: false,
        }
    }

    pub async fn recv(&mut self) -> Option<Result<WebSocketMessage, WebSocketStreamReceiveError>> {
        if let Some(message) = self.receiver.recv().await {
            return Some(Ok(message));
        }
        if self.terminal_reported {
            return None;
        }
        self.terminal_reported = true;
        match self.state.load() {
            StreamState::Finished => None,
            StreamState::Aborted => Some(Err(WebSocketStreamReceiveError::Aborted)),
            StreamState::Open | StreamState::ConnectionClosed => {
                Some(Err(WebSocketStreamReceiveError::ConnectionClosed))
            }
        }
    }
}

#[derive(Clone)]
pub struct WebSocketStreamReceiverHandle {
    receiver: Arc<Mutex<Option<WebSocketStreamReceiver>>>,
}

impl WebSocketStreamReceiverHandle {
    pub(crate) fn new(receiver: WebSocketStreamReceiver) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub fn take(&self) -> Option<WebSocketStreamReceiver> {
        self.receiver
            .lock()
            .expect("WebSocket stream receiver lock poisoned")
            .take()
    }
}

pub struct WebSocketConnected {
    pub connection_id: WebSocketConnectionId,
}

impl Event for WebSocketConnected {}

pub struct WebSocketMessageReceived {
    pub connection_id: WebSocketConnectionId,
    pub message: WebSocketMessage,
}

impl Event for WebSocketMessageReceived {}

pub struct WebSocketStreamOpened {
    pub connection_id: WebSocketConnectionId,
    pub stream_id: WebSocketStreamId,
    pub receiver: WebSocketStreamReceiverHandle,
}

impl Event for WebSocketStreamOpened {}

pub struct WebSocketDisconnected {
    pub connection_id: WebSocketConnectionId,
    pub reason: Option<WebSocketCloseReason>,
}

impl Event for WebSocketDisconnected {}

pub struct WebSocketProtocolFailed {
    pub connection_id: WebSocketConnectionId,
    pub error: WebSocketProtocolError,
}

impl Event for WebSocketProtocolFailed {}

pub(crate) struct WebSocketStream {
    pub(crate) sender: mpsc::Sender<WebSocketMessage>,
    pub(crate) state: Arc<SharedStreamState>,
}

impl PartialEq for WebSocketSender {
    fn eq(&self, other: &Self) -> bool {
        self.connection_id == other.connection_id
    }
}

impl Eq for WebSocketSender {}

impl Hash for WebSocketSender {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.connection_id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(value: u64) -> WebSocketSender {
        let (sender, _receiver) = mpsc::channel(1);
        WebSocketSender::new(
            WebSocketConnectionId::new(value),
            sender,
            Arc::new(SharedConnectionState::open()),
        )
    }

    #[test]
    fn default_classifier_recognizes_stream_envelopes() {
        let classified = JsonWebSocketMessageClassifier
            .classify(Message::Text(
                r#"{"mecs_stream":{"id":"prompt-1","phase":"chunk"},"payload":"hi"}"#.into(),
            ))
            .unwrap();

        assert!(matches!(
            classified,
            WebSocketMessageClassification::Stream {
                stream_id,
                phase: WebSocketStreamPhase::Chunk,
                ..
            } if stream_id.as_str() == "prompt-1"
        ));
    }

    #[test]
    fn messages_without_an_envelope_remain_ordinary() {
        assert!(matches!(
            JsonWebSocketMessageClassifier
                .classify(Message::Text("hello".into()))
                .unwrap(),
            WebSocketMessageClassification::Ordinary { .. }
        ));
    }

    #[test]
    fn stream_receiver_can_only_be_taken_once() {
        let (_sender, receiver) = mpsc::channel(1);
        let handle = WebSocketStreamReceiverHandle::new(WebSocketStreamReceiver::new(
            receiver,
            Arc::new(SharedStreamState::open()),
        ));

        assert!(handle.take().is_some());
        assert!(handle.take().is_none());
    }

    #[test]
    fn connection_registry_names_and_removes_senders_atomically() {
        let connections = WebSocketConnections::new();
        let first = sender(1);
        let second = sender(2);
        connections.insert(first.clone());
        connections.insert(second.clone());

        assert_eq!(connections.get_unnamed().len(), 2);
        connections
            .set_name(first.connection_id(), "frontend")
            .unwrap();
        assert_eq!(
            connections.name(first.connection_id()).as_deref(),
            Some("frontend")
        );
        assert_eq!(
            connections.get_by_name("frontend").unwrap().connection_id(),
            first.connection_id()
        );
        let unnamed = connections.get_unnamed();
        assert_eq!(unnamed.len(), 1);
        assert_eq!(unnamed[0].connection_id(), second.connection_id());
        assert!(matches!(
            connections.set_name(second.connection_id(), "frontend"),
            Err(WebSocketNameError::NameAlreadyExists { name }) if name == "frontend"
        ));

        connections.set_name(first.connection_id(), "").unwrap();
        assert!(connections.get_by_name("frontend").is_none());
        assert_eq!(connections.get_unnamed().len(), 2);
        connections.remove(first.connection_id());
        assert!(connections.get(first.connection_id()).is_none());
        let unnamed = connections.get_unnamed();
        assert_eq!(unnamed.len(), 1);
        assert_eq!(unnamed[0].connection_id(), second.connection_id());
    }

    #[test]
    fn naming_a_missing_connection_reports_its_id() {
        let connections = WebSocketConnections::new();
        let connection_id = WebSocketConnectionId::new(7);

        assert!(matches!(
            connections.set_name(connection_id, "frontend"),
            Err(WebSocketNameError::ConnectionNotFound {
                connection_id: missing_id
            }) if missing_id == connection_id
        ));
    }

    #[test]
    fn connection_registry_indexes_senders_by_type() {
        let connections = WebSocketConnections::new();
        let first = sender(1);
        let second = sender(2);
        connections.insert(first.clone());
        connections.insert(second.clone());

        assert_eq!(connections.get_all().len(), 2);
        assert!(connections.set_connection_type(first.connection_id(), "webui"));
        assert!(connections.set_connection_type(second.connection_id(), "cli"));
        assert_eq!(
            connections
                .connection_type(first.connection_id())
                .as_deref(),
            Some("webui")
        );
        let webui = connections.get_by_type("webui");
        assert_eq!(webui.len(), 1);
        assert_eq!(webui[0].connection_id(), first.connection_id());

        assert!(connections.set_connection_type(first.connection_id(), "cli"));
        assert!(connections.get_by_type("webui").is_empty());
        assert_eq!(connections.get_by_type("cli").len(), 2);

        connections.remove(first.connection_id());
        let cli = connections.get_by_type("cli");
        assert_eq!(cli.len(), 1);
        assert_eq!(cli[0].connection_id(), second.connection_id());
    }

    #[tokio::test]
    async fn aborted_stream_reports_its_terminal_state_once() {
        let (sender, receiver) = mpsc::channel(1);
        let state = Arc::new(SharedStreamState::open());
        let mut receiver = WebSocketStreamReceiver::new(receiver, Arc::clone(&state));
        state.abort();
        drop(sender);

        assert_eq!(
            receiver.recv().await,
            Some(Err(WebSocketStreamReceiveError::Aborted))
        );
        assert_eq!(receiver.recv().await, None);
    }

    #[tokio::test]
    async fn message_sender_delivers_one_direct_message_to_each_target() {
        let (first_tx, mut first_rx) = mpsc::channel(2);
        let (second_tx, mut second_rx) = mpsc::channel(2);
        let first = WebSocketSender::new(
            WebSocketConnectionId::new(1),
            first_tx,
            Arc::new(SharedConnectionState::open()),
        );
        let second = WebSocketSender::new(
            WebSocketConnectionId::new(2),
            second_tx,
            Arc::new(SharedConnectionState::open()),
        );

        let results = WebSocketMessageSender::new(
            vec![first, second],
            WebSocketMessage::Text("delta".into()),
        )
        .send()
        .await;

        assert!(results.iter().all(|(_, result)| result.is_ok()));
        assert_eq!(
            first_rx.recv().await,
            Some(WebSocketMessage::Text("delta".into()))
        );
        assert_eq!(
            second_rx.recv().await,
            Some(WebSocketMessage::Text("delta".into()))
        );
    }
}
