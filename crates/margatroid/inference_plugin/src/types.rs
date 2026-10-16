use std::fmt;
use std::sync::Arc;

use crate::error::{InferenceError, InferenceErrorKind};
pub use margatroid_types::message_types::StopReason;
use margatroid_types::message_types::{TokenUsage, ToolCall, UserImage};

#[derive(Clone, Debug)]
pub enum ProviderMessage {
    System {
        content: String,
    },
    User {
        content: String,
        images: Vec<UserImage>,
    },
    Assistant {
        reasoning: Option<String>,
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
        images: Vec<UserImage>,
    },
    Error {
        message: String,
    },
}
use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;

pub(crate) mod formats;
pub(crate) mod vendors;
use margatroid_types::inference_request_types::DomainRequest;
pub use vendors::Vendor;

pub(crate) const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_MESSAGES_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_TOOL_DESCRIPTION_BYTES: usize = 8 * 1024;
pub(crate) const MAX_TOOL_COUNT: usize = 256;
pub(crate) const MAX_STOP_COUNT: usize = 64;
pub(crate) const MAX_STOP_BYTES: usize = 256;
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(value: impl Into<String>) -> Result<Self, InferenceError> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelId,
                "model ID is empty, too long, or contains a control character",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InferenceParameters {
    temperature: Option<f32>,
    max_output_tokens: Option<u32>,
    top_p: Option<f32>,
    stop: Vec<String>,
}

impl InferenceParameters {
    pub fn new(
        temperature: Option<f32>,
        max_output_tokens: Option<u32>,
        top_p: Option<f32>,
        stop: Vec<String>,
    ) -> Self {
        Self {
            temperature,
            max_output_tokens,
            top_p,
            stop,
        }
    }

    pub fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    pub fn top_p(&self) -> Option<f32> {
        self.top_p
    }

    pub fn stop(&self) -> &[String] {
        &self.stop
    }

    pub fn validate(&self) -> Result<(), InferenceError> {
        if self
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
            || self
                .top_p
                .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
            || self.max_output_tokens.is_some_and(|value| value == 0)
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidParameters,
                "inference parameters are outside the supported range",
            ));
        }
        if self.stop.len() > MAX_STOP_COUNT
            || self
                .stop
                .iter()
                .any(|value| value.is_empty() || value.len() > MAX_STOP_BYTES)
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidParameters,
                "stop sequences are empty, too long, or too numerous",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ConfiguredModelRoute {
    pub(crate) model: String,
    pub(crate) context_window_tokens: u64,
    pub(crate) adapter: ErasedProviderAdapter,
    pub(crate) runtime_info: margatroid_types::model_components::ModelRuntimeInfo,
}

impl ConfiguredModelRoute {
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens
    }

    pub fn adapter(&self) -> &ErasedProviderAdapter {
        &self.adapter
    }
}

pub struct ProviderRouteInput<'a> {
    pub base_url: &'a Url,
    pub api_key: &'a str,
    pub thinking: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
}

impl<'a> ProviderRouteInput<'a> {
    pub fn base_url(&self) -> &Url {
        self.base_url
    }

    pub fn api_key(&self) -> &str {
        self.api_key
    }

    pub fn thinking(&self) -> Option<&str> {
        self.thinking
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort
    }
}

pub struct ProviderHttpRequest {
    pub(crate) method: Method,
    pub(crate) url: Url,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl ProviderHttpRequest {
    pub fn new(method: Method, url: Url, headers: HeaderMap, body: Vec<u8>) -> Self {
        Self {
            method,
            url,
            headers,
            body,
        }
    }
}

pub trait ProviderAdapter: Send + Sync + 'static {
    fn build_request(&self, input: DomainRequest) -> Result<ProviderHttpRequest, InferenceError>;

    fn begin_response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError>;
}

pub trait ProviderResponseAccumulator: Send + 'static {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>;

    fn finish(
        self: Box<Self>,
    ) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError>;
}

pub type ErasedProviderAdapter = Arc<dyn ProviderAdapter>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInferenceResponse {
    pub replay: Option<serde_json::Value>,
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderStreamDelta {
    Reasoning(String),
    Content(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelRouteDocument {
    pub(crate) models: Vec<ModelRouteConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelRouteConfig {
    pub(crate) id: String,
    pub(crate) model: String,
    pub provider: Vendor,
    pub base_url: String,
    pub api_key: String,
    pub(crate) api_type: formats::ApiFormat,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
    pub(crate) context_window: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThinkingMode {
    Enabled,
    Disabled,
}

impl ThinkingMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Default)]
pub(super) struct OpenAiAccumulator {
    buffer: Vec<u8>,
    event_data: Vec<u8>,
    total_bytes: usize,
    reasoning: String,
    content: String,
    tool_calls: Vec<OpenAiToolCallBuilder>,
    stop_reason: Option<StopReason>,
    usage: Option<TokenUsage>,
    saw_choice: bool,
    done: bool,
}

struct OpenAiToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    delta: Option<OpenAiDelta>,
    message: Option<OpenAiDelta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct OpenAiDelta {
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(alias = "prompt_tokens")]
    input_tokens: u64,
    #[serde(alias = "completion_tokens")]
    output_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiTokenDetails>,
    #[serde(default)]
    input_tokens_details: Option<OpenAiTokenDetails>,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OpenAiTokenDetails {
    #[serde(default)]
    cached_tokens: u64,
}

fn decode_token_usage(usage: OpenAiUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_hit_tokens: usage
            .prompt_tokens_details
            .or(usage.input_tokens_details)
            .map_or_else(
                || usage.prompt_cache_hit_tokens.unwrap_or(0),
                |details| details.cached_tokens,
            ),
    }
}

impl ProviderResponseAccumulator for OpenAiAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError> {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if self.total_bytes > 32 * 1024 * 1024
            || self.buffer.len().saturating_add(chunk.len()) > MAX_MESSAGE_BYTES
        {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "SSE buffer exceeds limit",
            ));
        }
        self.buffer.extend_from_slice(chunk);
        let mut text = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=index).collect::<Vec<_>>();
            text.extend(self.consume_line(&line[..line.len() - 1])?);
        }
        Ok(text)
    }

    fn finish(
        mut self: Box<Self>,
    ) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError> {
        let mut text = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            text.extend(self.consume_line(&line)?);
        }
        text.extend(self.consume_event()?);
        if !self.saw_choice || !self.done || self.stop_reason.is_none() {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "inference response ended before a complete choice",
            ));
        }
        let mut calls = Vec::with_capacity(self.tool_calls.len());
        let mut ids = std::collections::HashSet::new();
        for call in self.tool_calls {
            if call.id.is_empty()
                || call.name.is_empty()
                || !ids.insert(call.id.clone())
                || !serde_json::from_str::<serde_json::Value>(&call.arguments)
                    .is_ok_and(|v| v.is_object())
            {
                return Err(InferenceError::new(
                    InferenceErrorKind::ResponseIncomplete,
                    "inference tool call is missing an ID or name",
                ));
            }
            calls.push(ToolCall {
                id: call.id,
                tool_name: call.name,
                arguments: call.arguments,
            });
        }
        if self.content.is_empty()
            && calls.is_empty()
            && !matches!(
                self.stop_reason,
                Some(StopReason::ContentFilter | StopReason::Length)
            )
        {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "inference response contains neither content nor tool calls",
            ));
        }
        let reason = self.stop_reason.unwrap_or(if calls.is_empty() {
            StopReason::Completed
        } else {
            StopReason::ToolCalls
        });
        Ok((
            ProviderInferenceResponse {
                replay: None,
                reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
                content: (!self.content.is_empty()).then_some(self.content),
                tool_calls: calls,
                stop_reason: reason,
                usage: self.usage,
            },
            text,
        ))
    }
}

impl OpenAiAccumulator {
    fn consume_line(&mut self, line: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return self.consume_event();
        }
        if line.starts_with(b":") {
            return Ok(Vec::new());
        }
        if let Some(payload) = line.strip_prefix(b"data:") {
            let payload = payload.strip_prefix(b" ").unwrap_or(payload);
            if self.event_data.len().saturating_add(payload.len()) > MAX_MESSAGE_BYTES {
                return Err(InferenceError::new(
                    InferenceErrorKind::ResponseTooLarge,
                    "SSE event exceeds limit",
                ));
            }
            if !self.event_data.is_empty() {
                self.event_data.push(b'\n');
            }
            self.event_data.extend_from_slice(payload);
        }
        Ok(Vec::new())
    }

    fn consume_event(&mut self) -> Result<Vec<ProviderStreamDelta>, InferenceError> {
        let payload = std::mem::take(&mut self.event_data);
        if payload.is_empty() {
            return Ok(Vec::new());
        }
        if self.done {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseDecodeFailed,
                "data after SSE completion",
            ));
        }
        if payload == b"[DONE]" {
            self.done = true;
            return Ok(Vec::new());
        }
        let chunk = serde_json::from_slice::<OpenAiChunk>(&payload).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::ResponseDecodeFailed,
                "OpenAI-compatible response frame could not be decoded",
            )
        })?;
        self.consume_chunk(chunk)
    }

    fn consume_chunk(
        &mut self,
        chunk: OpenAiChunk,
    ) -> Result<Vec<ProviderStreamDelta>, InferenceError> {
        if let Some(usage) = chunk.usage {
            self.usage = Some(decode_token_usage(usage));
        }
        let mut output = Vec::new();
        for choice in chunk.choices {
            self.saw_choice = true;
            let delta = choice.delta.or(choice.message).unwrap_or_default();
            {
                if let Some(reasoning) = delta.reasoning_content.or(delta.reasoning) {
                    if self.reasoning.len().saturating_add(reasoning.len()) > MAX_MESSAGE_BYTES {
                        return Err(InferenceError::new(
                            InferenceErrorKind::ResponseDecodeFailed,
                            "inference response reasoning exceeds the size limit",
                        ));
                    }
                    self.reasoning.push_str(&reasoning);
                    output.push(ProviderStreamDelta::Reasoning(reasoning));
                }
            }
            if let Some(content) = delta.content {
                if self.content.len().saturating_add(content.len()) > MAX_MESSAGE_BYTES {
                    return Err(InferenceError::new(
                        InferenceErrorKind::ResponseDecodeFailed,
                        "inference response content exceeds the size limit",
                    ));
                }
                self.content.push_str(&content);
                output.push(ProviderStreamDelta::Content(content));
            }
            if let Some(tool_calls) = delta.tool_calls {
                for call in tool_calls {
                    let index = call.index.unwrap_or(self.tool_calls.len());
                    if index >= MAX_TOOL_COUNT {
                        return Err(InferenceError::new(
                            InferenceErrorKind::ResponseTooLarge,
                            "tool call index exceeds limit",
                        ));
                    }
                    while self.tool_calls.len() <= index {
                        self.tool_calls.push(OpenAiToolCallBuilder {
                            id: String::new(),
                            name: String::new(),
                            arguments: String::new(),
                        });
                    }
                    let target = &mut self.tool_calls[index];
                    if let Some(id) = call.id {
                        target.id.push_str(&id);
                    }
                    if target.id.len() > 256 {
                        return Err(InferenceError::new(
                            InferenceErrorKind::ResponseTooLarge,
                            "tool ID exceeds limit",
                        ));
                    }
                    if let Some(function) = call.function {
                        if let Some(name) = function.name {
                            target.name.push_str(&name);
                        }
                        if target.name.len() > 64 {
                            return Err(InferenceError::new(
                                InferenceErrorKind::ResponseTooLarge,
                                "tool name exceeds limit",
                            ));
                        }
                        if let Some(arguments) = function.arguments {
                            if target.arguments.len().saturating_add(arguments.len())
                                > MAX_MESSAGE_BYTES
                            {
                                return Err(InferenceError::new(
                                    InferenceErrorKind::ResponseDecodeFailed,
                                    "inference tool arguments exceed the size limit",
                                ));
                            }
                            target.arguments.push_str(&arguments);
                        }
                    }
                }
            }
            if let Some(reason) = choice.finish_reason {
                self.stop_reason = Some(parse_stop_reason(&reason));
            }
        }
        Ok(output)
    }
}

fn parse_stop_reason(value: &str) -> StopReason {
    match value {
        "stop" => StopReason::Completed,
        "tool_calls" | "function_call" => StopReason::ToolCalls,
        "length" => StopReason::Length,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::message_types::{TokenUsage, ToolCall};

    #[test]
    fn sse_multiline_crlf_utf8_and_content_filter_are_preserved() {
        let bytes = "event: message\r\ndata: {\"choices\":\r\ndata: [{\"delta\":{\"content\":\"你好\"},\"finish_reason\":\"content_filter\"}]}\r\n\r\ndata: [DONE]\r\n\r\n".as_bytes();
        let mut accumulator = OpenAiAccumulator::default();
        for byte in bytes {
            accumulator.push(&[*byte]).unwrap();
        }
        let (response, _) = Box::new(accumulator).finish().unwrap();
        assert_eq!(response.content.as_deref(), Some("你好"));
        assert_eq!(response.stop_reason, StopReason::ContentFilter);
    }

    #[test]
    fn unfinished_arguments_and_huge_tool_indices_are_rejected() {
        let mut accumulator = OpenAiAccumulator::default();
        accumulator.push(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"function\":{\"name\":\"echo\",\"arguments\":\"{\"}}]},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n").unwrap();
        assert_eq!(
            Box::new(accumulator).finish().unwrap_err().kind(),
            InferenceErrorKind::ResponseIncomplete
        );
        let mut accumulator = OpenAiAccumulator::default();
        assert!(accumulator
            .push(
                b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1000000000}]}}]}\n\n"
            )
            .is_err());
    }

    #[test]
    fn openai_usage_reads_cached_prompt_tokens() {
        let mut accumulator = OpenAiAccumulator::default();
        accumulator
            .push(br#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,"prompt_tokens_details":{"cached_tokens":75}}}

data: [DONE]

"#)
            .unwrap();
        let (response, _) = Box::new(accumulator).finish().unwrap();
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_hit_tokens: 75,
            })
        );
    }

    #[test]
    fn deepseek_usage_reads_prompt_cache_hit_tokens() {
        let mut accumulator = OpenAiAccumulator::default();
        accumulator
            .push(br#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":90,"completion_tokens":10,"total_tokens":100,"prompt_cache_hit_tokens":60}}

data: [DONE]

"#)
            .unwrap();
        let (response, _) = Box::new(accumulator).finish().unwrap();
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 90,
                output_tokens: 10,
                cache_hit_tokens: 60,
            })
        );
    }

    #[test]
    fn openai_accumulator_handles_split_sse() {
        let mut accumulator = OpenAiAccumulator::default();
        let first = br#"data: {"choices":[{"delta":{"role":"assistant","content":"he"},"finish_reason":null}]}

"#;
        let second = br#"data: {"choices":[{"delta":{"content":"llo"},"finish_reason":"stop"}]}

data: [DONE]

"#;
        let mut visible = accumulator.push(&first[..13]).unwrap();
        visible.extend(accumulator.push(&first[13..]).unwrap());
        visible.extend(accumulator.push(second).unwrap());
        let (response, trailing) = Box::new(accumulator).finish().unwrap();
        assert!(trailing.is_empty());
        assert_eq!(
            visible,
            [
                ProviderStreamDelta::Content("he".into()),
                ProviderStreamDelta::Content("llo".into()),
            ]
        );
        assert_eq!(response.reasoning, None);
        assert_eq!(response.content, Some("hello".into()));
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn openai_accumulator_reassembles_tool_call_fragments() {
        let mut accumulator = OpenAiAccumulator::default();
        let first_visible = accumulator
            .push(
                br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"echo","arguments":"{\"te"}}]},"finish_reason":null}]}

"#,
            )
            .unwrap();
        let second_visible = accumulator
            .push(
                br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"xt\":\"hi\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
            )
            .unwrap();
        assert!(first_visible.is_empty());
        assert!(second_visible.is_empty());
        let (response, trailing) = Box::new(accumulator).finish().unwrap();
        assert!(trailing.is_empty());
        assert_eq!(response.content, None);
        assert_eq!(
            response.tool_calls,
            vec![ToolCall {
                id: "call-1".into(),
                tool_name: "echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            }]
        );
        assert_eq!(response.stop_reason, StopReason::ToolCalls);
    }

    #[test]
    fn openai_accumulator_returns_text_from_the_trailing_line() {
        let mut accumulator = OpenAiAccumulator::default();
        let visible = accumulator
            .push(br#"data: {"choices":[{"delta":{"content":"tail"},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert!(visible.is_empty());

        assert_eq!(
            Box::new(accumulator).finish().unwrap_err().kind(),
            InferenceErrorKind::ResponseIncomplete
        );
    }

    #[test]
    fn deepseek_accumulator_separates_reasoning_and_content() {
        let mut accumulator = OpenAiAccumulator::default();
        let visible = accumulator
            .push(
                br#"data: {"choices":[{"delta":{"reasoning_content":"think","content":null},"finish_reason":null}]}

data: {"choices":[{"delta":{"reasoning":" more","content":"answer"},"finish_reason":"stop"}]}

data: [DONE]

"#,
            )
            .unwrap();
        let (response, trailing) = Box::new(accumulator).finish().unwrap();

        assert!(trailing.is_empty());
        assert_eq!(
            visible,
            [
                ProviderStreamDelta::Reasoning("think".into()),
                ProviderStreamDelta::Reasoning(" more".into()),
                ProviderStreamDelta::Content("answer".into()),
            ]
        );
        assert_eq!(response.reasoning, Some("think more".into()));
        assert_eq!(response.content, Some("answer".into()));
    }
}
