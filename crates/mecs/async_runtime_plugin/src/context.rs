use app_runtime_plugin::RuntimeEventSender;
use core_plugin::Event;

#[derive(Clone)]
pub struct AsyncContext {
    events: RuntimeEventSender,
}

impl AsyncContext {
    pub(crate) fn new(events: RuntimeEventSender) -> Self {
        Self { events }
    }

    pub fn send_event<E: Event>(&self, event: E) {
        self.events.send_event(event);
    }

    pub fn send_event_after<E: Event>(&self, event: E, delay: u64) {
        self.events.send_event_after(event, delay);
    }
}
