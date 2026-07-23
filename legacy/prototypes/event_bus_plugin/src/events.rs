use types::events::WorkspaceEvent;

#[derive(Clone, Debug)]
pub struct WorkspaceEventEmitted {
    pub channel: String,
    pub event: WorkspaceEvent,
}

impl WorkspaceEventEmitted {
    pub fn new(channel: impl Into<String>, event: WorkspaceEvent) -> Self {
        Self {
            channel: channel.into(),
            event,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBusPublishFailed {
    pub channel: String,
    pub message: String,
}
