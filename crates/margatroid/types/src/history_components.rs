use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// Unix epoch milliseconds; 0 means unknown (legacy data) or stamp on admission.
    /// Storage preserves the first admitted value across idempotent retries.
    #[serde(default)]
    pub created_at_ms: i64,
    pub key: String,
    pub source: String,
    pub turn: Option<String>,
    pub kind: String,
    /// Displayable text (or error message) for message rows, not serialized message JSON.
    pub content: String,
    /// Message discriminator and metadata; top-level text lives in `content`.
    /// Inject envelopes retain complete nested messages here.
    pub payload: serde_json::Value,
}

/// Split a message for history storage without duplicating its top-level text.
/// Nested Inject messages deliberately retain their complete domain representation.
/// An assistant without content has an empty display string, as does empty content.
pub fn message_history_parts(
    message: &crate::message_types::AgentMessage,
) -> (String, serde_json::Value) {
    use crate::message_types::AgentMessage;
    let mut payload = serde_json::to_value(message).expect("AgentMessage is JSON serializable");
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content, .. }
        | AgentMessage::ToolCall { content, .. } => content.clone(),
        AgentMessage::Assistant { content, .. } => content.clone().unwrap_or_default(),
        AgentMessage::Error { message } => message.clone(),
        AgentMessage::Inject { .. } => String::new(),
    };
    let object = payload
        .as_object_mut()
        .expect("AgentMessage serializes as an object");
    object.remove("content");
    if matches!(message, AgentMessage::Error { .. }) {
        object.remove("message");
    }
    (content, payload)
}

#[cfg(test)]
mod message_history_tests {
    use super::message_history_parts;
    use crate::message_types::AgentMessage;
    use serde_json::json;

    #[test]
    fn split_message_text_and_metadata_for_every_variant() {
        let cases = [
            (
                json!({"type":"system","content":"rules"}),
                "rules",
                json!({"type":"system"}),
            ),
            (
                json!({"type":"user","content":"hello"}),
                "hello",
                json!({"type":"user"}),
            ),
            (
                json!({"type":"assistant","content":"answer","reasoning":"thinking","tool_calls":[],"usage":null,"stop_reason":null}),
                "answer",
                json!({"type":"assistant","reasoning":"thinking","tool_calls":[],"usage":null,"stop_reason":null}),
            ),
            (
                json!({"type":"assistant","content":null,"reasoning":null,"tool_calls":[],"usage":null,"stop_reason":null}),
                "",
                json!({"type":"assistant","reasoning":null,"tool_calls":[],"usage":null,"stop_reason":null}),
            ),
            (
                json!({"type":"tool_call","resource_id":null,"tool_call_id":"call","tool_name":"tool","content":"result","failed":true}),
                "result",
                json!({"type":"tool_call","resource_id":null,"tool_call_id":"call","tool_name":"tool","failed":true}),
            ),
            (
                json!({"type":"error","message":"failed"}),
                "failed",
                json!({"type":"error"}),
            ),
        ];
        for (input, content, payload) in cases {
            let message: AgentMessage = serde_json::from_value(input).unwrap();
            assert_eq!(message_history_parts(&message), (content.into(), payload));
        }
    }

    #[test]
    fn empty_cancelled_assistant_projects_stop_reason_without_filler() {
        assert_eq!(
            message_history_parts(&AgentMessage::aborted_assistant()),
            (
                String::new(),
                json!({"type":"assistant","reasoning":null,"tool_calls":[],"usage":null,"stop_reason":"cancelled"}),
            )
        );
    }

    #[test]
    fn inject_retains_nested_content_and_error_structures() {
        let value = json!({"type":"inject","messages":[
            {"type":"inject","messages":[{"type":"user","content":"nested"}]},
            {"type":"error","message":"failure"}
        ]});
        let message = serde_json::from_value::<AgentMessage>(value.clone()).unwrap();
        assert_eq!(message_history_parts(&message), (String::new(), value));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryError {
    WriteFailed(String),
    KeyConflict(String),
    InvalidRecord(String),
    CleanupFailed(String),
    CommitUnknown(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for HistoryError {}

pub trait AgentHistoryStore: Send + Sync + 'static {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError>;
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError>;
}

#[derive(Clone)]
pub struct AgentHistoryHandle {
    inner: Arc<dyn AgentHistoryStore>,
}
impl std::fmt::Debug for AgentHistoryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHistoryHandle").finish_non_exhaustive()
    }
}
impl AgentHistoryHandle {
    pub fn new(store: impl AgentHistoryStore) -> Self {
        Self {
            inner: Arc::new(store),
        }
    }
    pub fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        self.inner.append_batch(records)
    }
    pub fn append(&self, record: HistoryRecord) -> Result<(), HistoryError> {
        self.append_batch(vec![record])
    }
    pub fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        self.inner.list()
    }
}
