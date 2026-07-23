use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventBusError {
    ChannelNotFound(String),
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventBusError::ChannelNotFound(channel) => {
                write!(f, "event bus channel `{channel}` is not registered")
            }
        }
    }
}

impl std::error::Error for EventBusError {}

pub struct EventBus {
    channels: RwLock<HashMap<String, broadcast::Sender<String>>>,
    channel_capacity: usize,
}

impl EventBus {
    pub const DEFAULT_CHANNEL_CAPACITY: usize = 128;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CHANNEL_CAPACITY)
    }

    pub fn with_capacity(channel_capacity: usize) -> Self {
        assert!(
            channel_capacity > 0,
            "event bus channel capacity must be > 0"
        );
        Self {
            channels: RwLock::new(HashMap::new()),
            channel_capacity,
        }
    }

    pub fn register(&self, channel: impl Into<String>) -> broadcast::Receiver<String> {
        let channel = channel.into();
        let mut channels = self
            .channels
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        channels
            .entry(channel)
            .or_insert_with(|| broadcast::channel(self.channel_capacity).0)
            .subscribe()
    }

    pub fn subscribe(&self, channel: &str) -> Option<broadcast::Receiver<String>> {
        self.channels
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(channel)
            .map(broadcast::Sender::subscribe)
    }

    pub fn publish(&self, channel: &str, data: String) -> Result<usize, EventBusError> {
        let channels = self
            .channels
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sender = channels
            .get(channel)
            .ok_or_else(|| EventBusError::ChannelNotFound(channel.to_string()))?;
        Ok(sender.send(data).unwrap_or(0))
    }

    pub fn unregister(&self, channel: &str) -> bool {
        self.channels
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(channel)
            .is_some()
    }

    pub fn channel_count(&self) -> usize {
        self.channels
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
