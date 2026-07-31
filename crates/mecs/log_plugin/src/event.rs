use core_plugin::{Event, World};

use crate::LogLevel;

pub const EVENT_LOG_TARGET: &str = "mecs::event_log";

#[derive(Debug)]
pub struct EventLog {
    pub level: LogLevel,
    pub message: String,
}

impl EventLog {
    pub fn new<Message>(level: LogLevel, message: Message) -> Self
    where
        Message: Into<String>,
    {
        Self {
            level,
            message: message.into(),
        }
    }
}

impl Event for EventLog {}

pub trait WorldEventLogExt {
    fn event_log<Message>(&self, level: LogLevel, message: Message)
    where
        Message: Into<String>;
}

impl WorldEventLogExt for World {
    fn event_log<Message>(&self, level: LogLevel, message: Message)
    where
        Message: Into<String>,
    {
        self.event_write().send_event(EventLog::new(level, message));
    }
}

pub(crate) fn event_log_system(world: &mut World) {
    for log in world.event_reader::<EventLog>() {
        match log.level {
            LogLevel::Off => {}
            LogLevel::Error => {
                tracing::error!(target: EVENT_LOG_TARGET, message = %log.message);
            }
            LogLevel::Warn => {
                tracing::warn!(target: EVENT_LOG_TARGET, message = %log.message);
            }
            LogLevel::Info => {
                tracing::info!(target: EVENT_LOG_TARGET, message = %log.message);
            }
            LogLevel::Debug => {
                tracing::debug!(target: EVENT_LOG_TARGET, message = %log.message);
            }
            LogLevel::Trace => {
                tracing::trace!(target: EVENT_LOG_TARGET, message = %log.message);
            }
        }
    }
}
