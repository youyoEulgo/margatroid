use std::fmt;

use app_runtime_plugin::AppControl;
use core_plugin::Event;
use tokio::sync::mpsc;

pub struct ExternalEventSender<E: Event> {
    sender: mpsc::Sender<E>,
    control: Option<AppControl>,
}

impl<E: Event> ExternalEventSender<E> {
    pub(crate) fn new(sender: mpsc::Sender<E>, control: Option<AppControl>) -> Self {
        Self { sender, control }
    }

    pub fn try_send(&self, event: E) -> Result<(), ExternalEventSendError<E>> {
        match self.sender.try_send(event) {
            Ok(()) => {
                if let Some(control) = &self.control {
                    control.wake();
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(event)) => Err(ExternalEventSendError::Full(event)),
            Err(mpsc::error::TrySendError::Closed(event)) => {
                Err(ExternalEventSendError::Closed(event))
            }
        }
    }

    pub fn max_capacity(&self) -> usize {
        self.sender.max_capacity()
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl<E: Event> Clone for ExternalEventSender<E> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            control: self.control.clone(),
        }
    }
}

pub enum ExternalEventSendError<E> {
    Full(E),
    Closed(E),
}

impl<E> ExternalEventSendError<E> {
    pub fn into_inner(self) -> E {
        match self {
            Self::Full(event) | Self::Closed(event) => event,
        }
    }
}

impl<E> fmt::Debug for ExternalEventSendError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(_) => formatter.write_str("ExternalEventSendError::Full(..)"),
            Self::Closed(_) => formatter.write_str("ExternalEventSendError::Closed(..)"),
        }
    }
}

impl<E> fmt::Display for ExternalEventSendError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(_) => formatter.write_str("external event queue is full"),
            Self::Closed(_) => formatter.write_str("external event queue is closed"),
        }
    }
}

impl<E> std::error::Error for ExternalEventSendError<E> {}
