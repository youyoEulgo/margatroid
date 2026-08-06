use core_plugin::{Entity, Event};
use margatroid_types::Message;

pub use margatroid_types::{ToolCall, ToolDefinition};

pub type InferenceStreamSender = tokio::sync::mpsc::Sender<String>;

#[derive(Clone, Debug)]
pub struct InferenceCommand {
    pub id: String,
    pub agent: Entity,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub stream: Option<InferenceStreamSender>,
}

impl Event for InferenceCommand {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_commands_are_events() {
        fn assert_event<EventType: Event>() {}

        assert_event::<InferenceCommand>();
    }
}
