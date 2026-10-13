use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Inject {
        messages: Vec<AgentMessage>,
    },
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
    },
    ToolCall {
        resource_id: Option<ResourceID>,
        tool_call_id: String,
        tool_name: String,
        content: String,
        #[serde(default)]
        failed: bool,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub message: AgentMessage,
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_role_round_trips_without_changing_assistant_tool_calls() {
        let message = AgentMessage::System {
            content: "system prompt".into(),
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["type"], "system");
        assert_eq!(
            serde_json::from_value::<AgentMessage>(value).unwrap(),
            message
        );
    }

    #[test]
    fn nested_inject_and_unknown_resource_failure_round_trip() {
        let message = AgentMessage::Inject {
            messages: vec![AgentMessage::Inject {
                messages: vec![AgentMessage::ToolCall {
                    resource_id: None,
                    tool_call_id: "call-1".into(),
                    tool_name: "missing".into(),
                    content: "not visible".into(),
                    failed: true,
                }],
            }],
        };
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<AgentMessage>(&encoded).unwrap(),
            message
        );
    }

    #[test]
    fn inference_response_keeps_length_and_usage() {
        let response = InferenceResponse {
            message: AgentMessage::Assistant {
                reasoning: None,
                content: Some("partial".into()),
                tool_calls: vec![],
                usage: Some(TokenUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                    cache_hit_tokens: 0,
                }),
            },
            stop_reason: StopReason::Length,
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serde_json::from_value::<InferenceResponse>(value).unwrap(),
            response
        );
    }
}
