use super::formats::{self, ApiFormat, ApiRequest};
use super::*;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};

/// Provider identity is independent of its selected wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    OpenAi,
    Anthropic,
    DeepSeek,
    OpenRouter,
}

impl Vendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }

    pub fn supports(self, format: ApiFormat) -> bool {
        matches!(
            (self, format),
            (
                Self::OpenAi,
                ApiFormat::ChatCompletions | ApiFormat::Responses
            ) | (Self::Anthropic, ApiFormat::Messages)
                | (
                    Self::DeepSeek,
                    ApiFormat::ChatCompletions | ApiFormat::Messages | ApiFormat::Responses
                )
                | (Self::OpenRouter, ApiFormat::ChatCompletions)
        )
    }
}

struct VendorAdapter {
    vendor: Vendor,
    format: ApiFormat,
    base_url: Url,
    api_key: String,
    thinking: Option<ThinkingMode>,
    reasoning_effort: Option<String>,
}

pub(crate) fn build_adapter(
    vendor: Vendor,
    format: ApiFormat,
    route: ProviderRouteInput<'_>,
) -> Result<ErasedProviderAdapter, InferenceError> {
    let invalid = |message| InferenceError::new(InferenceErrorKind::InvalidModelRoute, message);
    if !vendor.supports(format) {
        return Err(invalid("provider does not support the selected API format"));
    }
    if route.api_key.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(invalid(
            "provider API key contains an invalid control character",
        ));
    }
    if !(matches!(vendor, Vendor::DeepSeek | Vendor::OpenRouter)
        && format == ApiFormat::ChatCompletions)
        && (route.thinking.is_some() || route.reasoning_effort.is_some())
    {
        return Err(invalid(
            "thinking/reasoning_effort options are supported only for DeepSeek/OpenRouter chat_completions",
        ));
    }
    let thinking = route
        .thinking
        .map(|value| {
            ThinkingMode::parse(value)
                .ok_or_else(|| invalid("thinking must be enabled or disabled"))
        })
        .transpose()?;
    let reasoning_effort = match (thinking, route.reasoning_effort) {
        (Some(ThinkingMode::Enabled), Some("high")) => Some("high".to_owned()),
        (Some(ThinkingMode::Enabled), Some("max")) if vendor == Vendor::DeepSeek => Some("max".to_owned()),
        (Some(ThinkingMode::Enabled), None) => {
            return Err(invalid(
                "enabled thinking requires reasoning_effort",
            ))
        }
        (_, None) => None,
        _ => {
            return Err(invalid(
                "reasoning_effort requires enabled thinking: DeepSeek supports high/max; OpenRouter supports high",
            ))
        }
    };
    Ok(Arc::new(VendorAdapter {
        vendor,
        format,
        base_url: route.base_url.clone(),
        api_key: route.api_key.to_owned(),
        thinking,
        reasoning_effort,
    }))
}

impl VendorAdapter {
    fn process_request(&self, request: ApiRequest) -> Result<ProviderHttpRequest, InferenceError> {
        let mut body = match request {
            ApiRequest::ChatCompletions(request) => serde_json::to_value(request),
            ApiRequest::Messages(request) => serde_json::to_value(request),
            ApiRequest::Responses(request) => serde_json::to_value(request),
        }
        .map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "provider request could not be encoded",
            )
        })?;
        if self.vendor == Vendor::DeepSeek {
            if let Some(thinking) = self.thinking {
                body["thinking"] = serde_json::json!({"type": thinking.as_str()});
            }
            if let Some(effort) = &self.reasoning_effort {
                body["reasoning_effort"] = serde_json::json!(effort);
            }
            if self.format == ApiFormat::ChatCompletions {
                if let Some(messages) = body["messages"].as_array_mut() {
                    for message in messages {
                        if message["role"] != "assistant" {
                            continue;
                        }
                        let preserve = self.thinking != Some(ThinkingMode::Disabled)
                            || message.get("tool_calls").is_some();
                        if preserve {
                            message
                                .as_object_mut()
                                .unwrap()
                                .entry("reasoning_content")
                                .or_insert_with(|| serde_json::json!(""));
                        } else {
                            message.as_object_mut().unwrap().remove("reasoning_content");
                        }
                    }
                }
            }
        } else if self.vendor == Vendor::OpenRouter {
            if let Some(thinking) = self.thinking {
                body["reasoning"] =
                    serde_json::json!({"enabled": thinking == ThinkingMode::Enabled});
                if let Some(effort) = &self.reasoning_effort {
                    body["reasoning"]["effort"] = serde_json::json!(effort);
                }
            }
        } else if self.format == ApiFormat::ChatCompletions {
            if let Some(messages) = body["messages"].as_array_mut() {
                for message in messages {
                    if message["reasoning_content"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty())
                    {
                        return Err(InferenceError::new(
                            InferenceErrorKind::InvalidParameters,
                            "OpenAI chat_completions cannot replay historical reasoning",
                        ));
                    }
                    message
                        .as_object_mut()
                        .map(|object| object.remove("reasoning_content"));
                }
            }
        }
        let suffix = match (self.vendor, self.format) {
            (_, ApiFormat::ChatCompletions) => "/chat/completions",
            (_, ApiFormat::Messages) => "/messages",
            (_, ApiFormat::Responses) => "/responses",
        };
        let url = Url::parse(&format!(
            "{}{}",
            self.base_url.as_str().trim_end_matches('/'),
            suffix
        ))
        .map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "provider endpoint is invalid",
            )
        })?;
        let mut headers = HeaderMap::new();
        let header_error = |_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "provider authorization header is invalid",
            )
        };
        if self.format == ApiFormat::Messages {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&self.api_key).map_err(header_error)?,
            );
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        } else {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(header_error)?,
            );
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        let body = serde_json::to_vec(&body).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "provider request could not be encoded",
            )
        })?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "encoded provider request exceeds 32 MiB",
            ));
        }
        Ok(ProviderHttpRequest::new(Method::POST, url, headers, body))
    }
}

impl ProviderAdapter for VendorAdapter {
    fn build_request(&self, input: DomainRequest) -> Result<ProviderHttpRequest, InferenceError> {
        self.process_request(self.format.build_request(input)?)
    }

    fn begin_response(
        &self,
        status: StatusCode,
        _headers: &HeaderMap,
    ) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError> {
        if !status.is_success() {
            return Err(InferenceError::with_status(
                InferenceErrorKind::ResponseStatus,
                Some(status.as_u16()),
                "inference provider returned a non-success status",
            ));
        }
        Ok(match self.format {
            ApiFormat::ChatCompletions => Box::new(OpenAiAccumulator::default()),
            ApiFormat::Messages => formats::new_messages_accumulator(),
            ApiFormat::Responses => formats::new_responses_accumulator(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::{
        inference_request_types::DomainRequestConfig, message_types::AgentMessage,
    };

    fn request() -> DomainRequest {
        DomainRequest {
            messages: vec![AgentMessage::User {
                content: "hello".into(),
                images: vec![],
            }],
            tools: vec![],
            config: DomainRequestConfig {
                model: "model".into(),
                max_output_tokens: Some(100),
                ..Default::default()
            },
        }
    }

    #[test]
    fn final_serialized_request_counts_base64_and_envelope_overhead() {
        let base = Url::parse("http://127.0.0.1:1/v1").unwrap();
        let adapter = build_adapter(
            Vendor::OpenAi,
            ApiFormat::ChatCompletions,
            ProviderRouteInput {
                base_url: &base,
                api_key: "test",
                thinking: None,
                reasoning_effort: None,
            },
        )
        .unwrap();
        let image = margatroid_types::message_types::UserImage {
            media_type: "image/png".into(),
            data: format!(
                "iVBORw0KGgoA{}",
                "AAAA".repeat((4 * 1024 * 1024 + 512 * 1024 - 9) / 3)
            ),
        };
        let mut input = request();
        input.messages = vec![AgentMessage::User {
            content: String::new(),
            images: vec![image; 4],
        }];
        let small = adapter.build_request(input.clone()).unwrap();
        let remaining = MAX_REQUEST_BYTES - small.body.len();
        input
            .tools
            .push(margatroid_types::message_types::ToolDefinition {
                name: "echo".into(),
                description: "a".repeat(remaining),
                input_schema: serde_json::json!({"type":"object"}),
            });
        let error = adapter.build_request(input).err().unwrap();
        assert_eq!(error.kind, InferenceErrorKind::InvalidMessages);
        assert!(!error.to_string().contains("iVBORw0KGgoA"));
    }

    #[test]
    fn matrix_is_explicit_and_auth_and_endpoints_follow_format() {
        let base = Url::parse("https://example.test/v1").unwrap();
        for vendor in [
            Vendor::OpenAi,
            Vendor::Anthropic,
            Vendor::DeepSeek,
            Vendor::OpenRouter,
        ] {
            for format in [
                ApiFormat::ChatCompletions,
                ApiFormat::Messages,
                ApiFormat::Responses,
            ] {
                let adapter = build_adapter(
                    vendor,
                    format,
                    ProviderRouteInput {
                        base_url: &base,
                        api_key: "secret",
                        thinking: None,
                        reasoning_effort: None,
                    },
                );
                let expected = match vendor {
                    Vendor::OpenAi => format != ApiFormat::Messages,
                    Vendor::Anthropic => format == ApiFormat::Messages,
                    Vendor::DeepSeek => true,
                    Vendor::OpenRouter => format == ApiFormat::ChatCompletions,
                };
                assert_eq!(vendor.supports(format), expected);
                if !expected {
                    assert!(adapter.is_err());
                    continue;
                }
                let http = adapter.unwrap().build_request(request()).unwrap();
                let suffix = match (vendor, format) {
                    (_, ApiFormat::ChatCompletions) => "/chat/completions",
                    (_, ApiFormat::Messages) => "/messages",
                    (_, ApiFormat::Responses) => "/responses",
                };
                assert_eq!(
                    http.url.as_str(),
                    format!("https://example.test/v1{suffix}")
                );
                assert_eq!(http.method, Method::POST);
                if format == ApiFormat::Messages {
                    assert_eq!(http.headers["x-api-key"], "secret");
                    assert_eq!(http.headers["anthropic-version"], "2023-06-01");
                    assert!(!http.headers.contains_key(AUTHORIZATION));
                } else {
                    assert_eq!(http.headers[AUTHORIZATION], "Bearer secret");
                }
                let body: serde_json::Value = serde_json::from_slice(&http.body).unwrap();
                assert_eq!(body["model"], "model");
                assert_eq!(body["stream"], true);
            }
        }
    }

    #[test]
    fn legacy_protocol_names_and_unknown_provider_options_are_rejected() {
        for legacy in ["openai", "deepseek"] {
            assert!(serde_json::from_value::<ApiFormat>(serde_json::json!(legacy)).is_err());
        }
        for vendor in [
            Vendor::OpenAi,
            Vendor::Anthropic,
            Vendor::DeepSeek,
            Vendor::OpenRouter,
        ] {
            let encoded = serde_json::to_value(vendor).unwrap();
            assert_eq!(encoded, vendor.as_str());
        }
        let valid = serde_json::json!({"models":[{"id":"route","model":"model","provider":"openai","api_type":"chat_completions","base_url":"https://example.test/v1","api_key":"secret"}]});
        assert!(serde_json::from_value::<ModelRouteDocument>(valid.clone()).is_ok());
        let mut missing_provider = valid.clone();
        missing_provider["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("provider");
        assert!(serde_json::from_value::<ModelRouteDocument>(missing_provider).is_err());
        let mut unknown_option = valid;
        unknown_option["models"][0]["ignored_option"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ModelRouteDocument>(unknown_option).is_err());
        let base = Url::parse("https://example.test/v1").unwrap();
        for vendor in [Vendor::OpenAi] {
            assert!(build_adapter(
                vendor,
                ApiFormat::ChatCompletions,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking: Some("enabled"),
                    reasoning_effort: Some("high"),
                }
            )
            .is_err());
        }
    }

    #[test]
    fn deepseek_nonchat_formats_reject_unverified_tuning() {
        let base = Url::parse("https://example.test/v1").unwrap();
        for format in [ApiFormat::Messages, ApiFormat::Responses] {
            for (thinking, reasoning_effort) in [
                (Some("enabled"), Some("high")),
                (Some("disabled"), None),
                (None, Some("high")),
            ] {
                let error = build_adapter(
                    Vendor::DeepSeek,
                    format,
                    ProviderRouteInput {
                        base_url: &base,
                        api_key: "secret",
                        thinking,
                        reasoning_effort,
                    },
                )
                .err()
                .unwrap();
                assert_eq!(error.kind(), InferenceErrorKind::InvalidModelRoute);
            }
            assert!(build_adapter(
                Vendor::DeepSeek,
                format,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking: None,
                    reasoning_effort: None,
                }
            )
            .is_ok());
        }
    }

    #[test]
    fn openrouter_uses_explicit_reasoning_options_and_preserves_history() {
        let base = Url::parse("https://example.test/v1").unwrap();
        for (thinking, effort, expected) in [
            (None, None, None),
            (
                Some("enabled"),
                Some("high"),
                Some(serde_json::json!({"enabled":true,"effort":"high"})),
            ),
            (
                Some("disabled"),
                None,
                Some(serde_json::json!({"enabled":false})),
            ),
        ] {
            let adapter = build_adapter(
                Vendor::OpenRouter,
                ApiFormat::ChatCompletions,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking,
                    reasoning_effort: effort,
                },
            )
            .unwrap();
            let mut input = request();
            input.messages.push(AgentMessage::Assistant {
                replay: None,
                reasoning: Some("thought".into()),
                content: Some("answer".into()),
                tool_calls: vec![],
                usage: None,
                stop_reason: None,
            });
            let http = adapter.build_request(input.clone()).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&http.body).unwrap();
            assert_eq!(body.get("reasoning"), expected.as_ref());
            assert!(body.get("thinking").is_none());
            assert!(body.get("reasoning_effort").is_none());
            assert_eq!(body["messages"][1]["reasoning_content"], "thought");
            let openai = build_adapter(
                Vendor::OpenAi,
                ApiFormat::ChatCompletions,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking: None,
                    reasoning_effort: None,
                },
            )
            .unwrap();
            assert_eq!(
                openai.build_request(input).err().unwrap().kind(),
                InferenceErrorKind::InvalidParameters
            );
        }
        for (thinking, effort) in [
            (Some("enabled"), Some("max")),
            (Some("enabled"), None),
            (Some("disabled"), Some("high")),
            (None, Some("high")),
            (Some("bad"), None),
        ] {
            assert!(build_adapter(
                Vendor::OpenRouter,
                ApiFormat::ChatCompletions,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking,
                    reasoning_effort: effort,
                }
            )
            .is_err());
        }
    }

    #[test]
    fn deepseek_tuning_is_validated_and_keeps_tool_reasoning() {
        let base = Url::parse("https://example.test/v1").unwrap();
        for (thinking, effort, valid) in [
            (None, None, true),
            (Some("enabled"), Some("high"), true),
            (Some("enabled"), Some("max"), true),
            (Some("disabled"), None, true),
            (Some("enabled"), None, false),
            (None, Some("high"), false),
            (Some("disabled"), Some("high"), false),
            (Some("bogus"), None, false),
            (Some("enabled"), Some("low"), false),
        ] {
            let adapter = build_adapter(
                Vendor::DeepSeek,
                ApiFormat::ChatCompletions,
                ProviderRouteInput {
                    base_url: &base,
                    api_key: "secret",
                    thinking,
                    reasoning_effort: effort,
                },
            );
            assert_eq!(adapter.is_ok(), valid);
            if !valid {
                continue;
            }
            let mut input = request();
            input.messages.push(AgentMessage::Assistant {
                replay: None,
                reasoning: Some("thought".into()),
                content: Some("answer".into()),
                tool_calls: vec![],
                usage: None,
                stop_reason: None,
            });
            input.messages.push(AgentMessage::Assistant {
                replay: None,
                reasoning: None,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    tool_name: "echo".into(),
                    arguments: "{}".into(),
                }],
                usage: None,
                stop_reason: None,
            });
            input.messages.push(AgentMessage::ToolCall {
                resource_id: None,
                tool_call_id: "call-1".into(),
                tool_name: "echo".into(),
                content: "ok".into(),
                images: vec![],
                failed: false,
            });
            let http = adapter.unwrap().build_request(input).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&http.body).unwrap();
            assert_eq!(body["messages"][2]["reasoning_content"], "");
            if thinking == Some("disabled") {
                assert!(body["messages"][1].get("reasoning_content").is_none());
            } else {
                assert_eq!(body["messages"][1]["reasoning_content"], "thought");
            }
            assert_eq!(body["reasoning_effort"].as_str(), effort);
            assert_eq!(body["thinking"]["type"].as_str(), thinking);
        }
    }
}
