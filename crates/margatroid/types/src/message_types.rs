use base64::{engine::general_purpose::STANDARD, Engine as _};
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};

pub const MAX_USER_IMAGES: usize = 8;
pub const MAX_USER_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_USER_IMAGES_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserImage {
    pub media_type: String,
    pub data: String,
}

fn deserialize_user_images<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<UserImage>, D::Error> {
    let images = Vec::<UserImage>::deserialize(deserializer)?;
    validate_user_images(&images).map_err(serde::de::Error::custom)?;
    Ok(images)
}

pub fn validate_user_images(images: &[UserImage]) -> Result<(), String> {
    if images.len() > MAX_USER_IMAGES {
        return Err(format!(
            "user message permits at most {MAX_USER_IMAGES} images"
        ));
    }
    let mut total = 0usize;
    for (index, image) in images.iter().enumerate() {
        let error = |reason: &str| format!("user image {}: {reason}", index + 1);
        if !matches!(
            image.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return Err(error(
                "unsupported media type; expected image/png, image/jpeg, image/gif or image/webp",
            ));
        }
        if image.data.len() > MAX_USER_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(error("decoded size exceeds 5 MiB per-image limit"));
        }
        if image.data.is_empty() || image.data.len() % 4 != 0 {
            return Err(error(
                "expected canonical standard base64 with required padding (not a data URL)",
            ));
        }
        let padding = image
            .data
            .as_bytes()
            .iter()
            .rev()
            .take_while(|&&byte| byte == b'=')
            .count();
        if padding > 2 {
            return Err(error("invalid canonical standard base64 padding"));
        }
        let decoded_len = image.data.len() / 4 * 3 - padding;
        if decoded_len > MAX_USER_IMAGE_BYTES {
            return Err(error("decoded size exceeds 5 MiB per-image limit"));
        }
        total += decoded_len;
        if total > MAX_USER_IMAGES_BYTES {
            return Err(error("total decoded size exceeds 20 MiB per-message limit"));
        }
    }
    for (index, image) in images.iter().enumerate() {
        let error = |reason: &str| format!("user image {}: {reason}", index + 1);
        let bytes = STANDARD
            .decode(&image.data)
            .map_err(|_| error("invalid canonical standard base64"))?;
        let valid_magic = match image.media_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            _ => false,
        };
        if !valid_magic {
            return Err(error("image signature does not match media type"));
        }
    }
    Ok(())
}

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
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "deserialize_user_images"
        )]
        images: Vec<UserImage>,
    },
    Assistant {
        reasoning: Option<String>,
        /// Opaque, API-format-tagged reasoning state required to replay this turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay: Option<serde_json::Value>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
        #[serde(default)]
        stop_reason: Option<StopReason>,
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

impl AgentMessage {
    pub fn aborted_assistant() -> Self {
        Self::Assistant {
            reasoning: None,
            replay: None,
            content: None,
            tool_calls: Vec::new(),
            usage: None,
            stop_reason: Some(StopReason::Cancelled),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Cancelled,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub message: AgentMessage,
    pub stop_reason: StopReason,
}

impl InferenceResponse {
    pub fn aborted() -> Self {
        Self {
            message: AgentMessage::aborted_assistant(),
            stop_reason: StopReason::Cancelled,
        }
    }
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
    fn legacy_assistant_defaults_stop_reason_and_abort_round_trips() {
        let legacy: AgentMessage = serde_json::from_str(
            r#"{"type":"assistant","content":"old","reasoning":null,"tool_calls":[],"usage":null}"#,
        )
        .unwrap();
        assert!(matches!(
            legacy,
            AgentMessage::Assistant {
                stop_reason: None,
                replay: None,
                ..
            }
        ));
        let aborted = InferenceResponse::aborted();
        assert_eq!(aborted.stop_reason, StopReason::Cancelled);
        assert_eq!(
            serde_json::from_value::<InferenceResponse>(serde_json::to_value(&aborted).unwrap())
                .unwrap(),
            aborted
        );
        assert!(matches!(aborted.message, AgentMessage::Assistant {
            reasoning: None,
            replay: None, content: None, tool_calls, usage: None,
            stop_reason: Some(StopReason::Cancelled),
        } if tool_calls.is_empty()));
    }

    #[test]
    fn assistant_replay_round_trips_and_none_is_omitted() {
        let mut message = AgentMessage::aborted_assistant();
        assert!(serde_json::to_value(&message)
            .unwrap()
            .get("replay")
            .is_none());
        if let AgentMessage::Assistant { replay, .. } = &mut message {
            *replay = Some(
                serde_json::json!({"api_format":"messages","items":[{"type":"redacted_thinking","data":"opaque"}]}),
            );
        }
        assert_eq!(
            serde_json::from_value::<AgentMessage>(serde_json::to_value(&message).unwrap())
                .unwrap(),
            message
        );
    }

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
                replay: None,
                content: Some("partial".into()),
                tool_calls: vec![],
                stop_reason: Some(StopReason::Length),
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
