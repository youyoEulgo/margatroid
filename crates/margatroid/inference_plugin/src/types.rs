use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use core_plugin::{Component, Entity, Resource};

use crate::error::{InferenceError, InferenceErrorKind};
use margatroid_types::{Message, TokenUsage, ToolCall, ToolDefinition};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};

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

    pub(crate) fn validate(&self) -> Result<(), InferenceError> {
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
pub struct AgentInferenceSnapshot {
    pub(crate) model: ModelId,
    pub(crate) context_window_tokens: u64,
    pub(crate) parameters: InferenceParameters,
    pub(crate) workspace: Entity,
    pub(crate) source_image: Entity,
}

impl AgentInferenceSnapshot {
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens
    }

    pub fn parameters(&self) -> &InferenceParameters {
        &self.parameters
    }

    pub fn workspace(&self) -> Entity {
        self.workspace
    }

    pub fn source_image(&self) -> Entity {
        self.source_image
    }
}

impl Component for AgentInferenceSnapshot {}

#[derive(Clone)]
pub struct ConfiguredModelRoute {
    pub(crate) model: String,
    pub(crate) context_window_tokens: u64,
    pub(crate) adapter: ErasedProviderAdapter,
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

#[derive(Clone)]
pub struct WorkspaceModelRoutes {
    pub(crate) routes: HashMap<ModelId, ConfiguredModelRoute>,
}

impl WorkspaceModelRoutes {
    pub fn get(&self, id: &ModelId) -> Option<ConfiguredModelRoute> {
        self.routes.get(id).cloned()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[derive(Default)]
pub struct WorkspaceModelRoutesRegistry {
    routes: HashMap<Entity, WorkspaceModelRoutes>,
}

impl Resource for WorkspaceModelRoutesRegistry {}

impl WorkspaceModelRoutesRegistry {
    pub fn get(&self, workspace: Entity) -> Option<&WorkspaceModelRoutes> {
        self.routes.get(&workspace)
    }

    pub fn insert(&mut self, workspace: Entity, routes: WorkspaceModelRoutes) {
        self.routes.insert(workspace, routes);
    }

    pub fn remove(&mut self, workspace: Entity) {
        self.routes.remove(&workspace);
    }
}

pub struct ProviderInput<'a> {
    model: &'a str,
    parameters: &'a InferenceParameters,
    messages: &'a [Message],
    tools: &'a [ToolDefinition],
}

impl<'a> ProviderInput<'a> {
    pub(crate) fn new(
        model: &'a str,
        parameters: &'a InferenceParameters,
        messages: &'a [Message],
        tools: &'a [ToolDefinition],
    ) -> Self {
        Self {
            model,
            parameters,
            messages,
            tools,
        }
    }

    pub fn model(&self) -> &str {
        self.model
    }

    pub fn parameters(&self) -> &InferenceParameters {
        self.parameters
    }

    pub fn messages(&self) -> &[Message] {
        self.messages
    }

    pub fn tools(&self) -> &[ToolDefinition] {
        self.tools
    }
}

pub struct ProviderRouteInput<'a> {
    pub(crate) provider: Option<&'a str>,
    pub(crate) base_url: &'a Url,
    pub(crate) api_key: &'a str,
    pub(crate) thinking: Option<&'a str>,
    pub(crate) reasoning_effort: Option<&'a str>,
}

impl<'a> ProviderRouteInput<'a> {
    pub fn provider(&self) -> Option<&str> {
        self.provider
    }

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
    fn build_request(
        &self,
        input: ProviderInput<'_>,
    ) -> Result<ProviderHttpRequest, InferenceError>;

    fn begin_response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError>;
}

pub trait ProviderAdapterFactory: Send + Sync + 'static {
    fn build(&self, route: ProviderRouteInput<'_>)
        -> Result<ErasedProviderAdapter, InferenceError>;
}

pub trait ProviderResponseAccumulator: Send + 'static {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError>;

    fn finish(
        self: Box<Self>,
    ) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError>;
}

pub type ErasedProviderAdapter = Arc<dyn ProviderAdapter>;
pub type ErasedProviderAdapterFactory = Arc<dyn ProviderAdapterFactory>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInferenceResponse {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRoutesReloaded {
    pub route_count: usize,
}

#[derive(Deserialize)]
pub(crate) struct ModelRouteDocument {
    pub(crate) models: Vec<ModelRouteConfig>,
}

#[derive(Deserialize)]
pub(crate) struct ModelRouteConfig {
    pub(crate) id: String,
    pub(crate) model: String,
    pub(crate) provider: Option<String>,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) api_type: String,
    pub(crate) thinking: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) context_window: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiAdapterFactory;

impl OpenAiAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProviderAdapterFactory for OpenAiAdapterFactory {
    fn build(
        &self,
        route: ProviderRouteInput<'_>,
    ) -> Result<ErasedProviderAdapter, InferenceError> {
        if route.thinking().is_some() || route.reasoning_effort().is_some() {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "OpenAI model route cannot contain DeepSeek reasoning options",
            ));
        }
        if route.api_key().bytes().any(|byte| byte.is_ascii_control()) {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "provider API key contains an invalid control character",
            ));
        }
        Ok(Arc::new(OpenAiAdapter {
            base_url: route.base_url().clone(),
            api_key: route.api_key().to_owned(),
        }))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeepSeekAdapterFactory;

impl DeepSeekAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProviderAdapterFactory for DeepSeekAdapterFactory {
    fn build(
        &self,
        route: ProviderRouteInput<'_>,
    ) -> Result<ErasedProviderAdapter, InferenceError> {
        if route.api_key().bytes().any(|byte| byte.is_ascii_control()) {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "provider API key contains an invalid control character",
            ));
        }
        let thinking = match route.thinking() {
            None | Some("disabled") => false,
            Some("enabled") => true,
            Some(_) => {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidModelRoute,
                    "DeepSeek thinking must be enabled or disabled",
                ))
            }
        };
        let reasoning_effort =
            match (thinking, route.reasoning_effort()) {
                (true, Some(value @ ("high" | "max"))) => Some(value.to_owned()),
                (true, None) => {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidModelRoute,
                        "enabled DeepSeek thinking requires reasoning_effort",
                    ))
                }
                (false, None) => None,
                _ => return Err(InferenceError::new(
                    InferenceErrorKind::InvalidModelRoute,
                    "DeepSeek reasoning_effort must be high or max and requires enabled thinking",
                )),
            };
        Ok(Arc::new(DeepSeekAdapter {
            base_url: route.base_url().clone(),
            api_key: route.api_key().to_owned(),
            thinking,
            reasoning_effort,
        }))
    }
}

struct DeepSeekAdapter {
    base_url: Url,
    api_key: String,
    thinking: bool,
    reasoning_effort: Option<String>,
}

impl ProviderAdapter for DeepSeekAdapter {
    fn build_request(
        &self,
        input: ProviderInput<'_>,
    ) -> Result<ProviderHttpRequest, InferenceError> {
        let endpoint = format!(
            "{}/chat/completions",
            self.base_url.as_str().trim_end_matches('/')
        );
        let url = Url::parse(&endpoint).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "DeepSeek chat endpoint is invalid",
            )
        })?;
        let body =
            OpenAiRequest::from_deepseek_input(input, self.thinking, self.reasoning_effort.clone());
        let body = serde_json::to_vec(&body).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "DeepSeek request could not be encoded",
            )
        })?;
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|_| {
                InferenceError::new(
                    InferenceErrorKind::RequestBuildFailed,
                    "provider authorization header is invalid",
                )
            })?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        Ok(ProviderHttpRequest::new(Method::POST, url, headers, body))
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
                "DeepSeek returned a non-success status",
            ));
        }
        Ok(Box::new(DeepSeekAccumulator::default()))
    }
}

struct OpenAiAdapter {
    base_url: Url,
    api_key: String,
}

impl ProviderAdapter for OpenAiAdapter {
    fn build_request(
        &self,
        input: ProviderInput<'_>,
    ) -> Result<ProviderHttpRequest, InferenceError> {
        let endpoint = format!(
            "{}/chat/completions",
            self.base_url.as_str().trim_end_matches('/')
        );
        let url = Url::parse(&endpoint).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "OpenAI-compatible chat endpoint is invalid",
            )
        })?;
        let body = OpenAiRequest::from_input(input);
        let body = serde_json::to_vec(&body).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestBuildFailed,
                "OpenAI-compatible request could not be encoded",
            )
        })?;
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|_| {
                InferenceError::new(
                    InferenceErrorKind::RequestBuildFailed,
                    "provider authorization header is invalid",
                )
            })?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        Ok(ProviderHttpRequest::new(Method::POST, url, headers, body))
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
        Ok(Box::new(OpenAiAccumulator::default()))
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    stream: bool,
    stream_options: OpenAiStreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<DeepSeekThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

impl OpenAiRequest {
    fn from_input(input: ProviderInput<'_>) -> Self {
        let messages = input.messages().iter().map(openai_message).collect();
        let tools = input
            .tools()
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
        Self {
            model: input.model().to_owned(),
            messages,
            tools,
            stream: true,
            stream_options: OpenAiStreamOptions {
                include_usage: true,
            },
            temperature: input.parameters().temperature(),
            max_tokens: input.parameters().max_output_tokens(),
            top_p: input.parameters().top_p(),
            stop: input.parameters().stop().to_vec(),
            thinking: None,
            reasoning_effort: None,
        }
    }

    fn from_deepseek_input(
        input: ProviderInput<'_>,
        thinking: bool,
        reasoning_effort: Option<String>,
    ) -> Self {
        let messages = input.messages().iter().map(deepseek_message).collect();
        let tools = input
            .tools()
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
        Self {
            model: input.model().to_owned(),
            messages,
            tools,
            stream: true,
            stream_options: OpenAiStreamOptions {
                include_usage: true,
            },
            temperature: input.parameters().temperature(),
            max_tokens: input.parameters().max_output_tokens(),
            top_p: input.parameters().top_p(),
            stop: input.parameters().stop().to_vec(),
            thinking: thinking.then_some(DeepSeekThinking {
                thinking_type: "enabled",
            }),
            reasoning_effort,
        }
    }
}

#[derive(Serialize)]
struct DeepSeekThinking {
    #[serde(rename = "type")]
    thinking_type: &'static str,
}

fn openai_message(message: &Message) -> serde_json::Value {
    match message {
        Message::System { content } => serde_json::json!({"role":"system", "content":content}),
        Message::User { content } => serde_json::json!({"role":"user", "content":content}),
        Message::Error { message } => serde_json::json!({"role":"user", "content":message}),
        Message::Assistant {
            content,
            tool_calls,
            ..
        } => {
            let mut value = serde_json::json!({"role": "assistant", "content": content});
            if !tool_calls.is_empty() {
                value["tool_calls"] = serde_json::Value::Array(
                    tool_calls
                        .iter()
                        .map(|call| {
                            serde_json::json!({
                                "id": call.id,
                                "type": "function",
                                "function": {"name": call.tool_name, "arguments": call.arguments}
                            })
                        })
                        .collect(),
                );
            }
            value
        }
        Message::Tool {
            tool_call_id,
            content,
            ..
        } => serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

fn deepseek_message(message: &Message) -> serde_json::Value {
    match message {
        Message::Assistant {
            reasoning,
            content,
            tool_calls,
        } => {
            let mut message = serde_json::json!({"role": "assistant", "content": content});
            if !tool_calls.is_empty() {
                message["tool_calls"] = serde_json::Value::Array(
                    tool_calls
                        .iter()
                        .map(|call| {
                            serde_json::json!({
                                "id": call.id,
                                "type": "function",
                                "function": {"name": call.tool_name, "arguments": call.arguments}
                            })
                        })
                        .collect(),
                );
            }
            if !tool_calls.is_empty() {
                message["reasoning_content"] =
                    serde_json::Value::String(reasoning.clone().unwrap_or_default());
            }
            message
        }
        _ => openai_message(message),
    }
}

#[derive(Default)]
struct OpenAiAccumulator {
    buffer: Vec<u8>,
    reasoning: String,
    content: String,
    tool_calls: Vec<OpenAiToolCallBuilder>,
    stop_reason: Option<StopReason>,
    usage: Option<TokenUsage>,
    saw_choice: bool,
    done: bool,
    capture_reasoning: bool,
}

struct OpenAiToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiChunk {
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
        if !self.saw_choice || !self.done && self.stop_reason.is_none() {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "inference response ended before a complete choice",
            ));
        }
        let mut calls = Vec::with_capacity(self.tool_calls.len());
        for call in self.tool_calls {
            if call.id.is_empty() || call.name.is_empty() {
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
        if self.content.is_empty() && calls.is_empty() {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "inference response contains neither content nor tool calls",
            ));
        }
        let reason = if calls.is_empty() {
            self.stop_reason.unwrap_or(StopReason::Completed)
        } else {
            StopReason::ToolCalls
        };
        Ok((
            ProviderInferenceResponse {
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
        if line.is_empty() || line.starts_with(b":") {
            return Ok(Vec::new());
        }
        let payload = line
            .strip_prefix(b"data:")
            .map(|payload| payload.strip_prefix(b" ").unwrap_or(payload))
            .unwrap_or(line);
        if payload == b"[DONE]" {
            self.done = true;
            return Ok(Vec::new());
        }
        let chunk = serde_json::from_slice::<OpenAiChunk>(payload).map_err(|_| {
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
            if self.capture_reasoning {
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
                    if let Some(function) = call.function {
                        if let Some(name) = function.name {
                            target.name.push_str(&name);
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

#[derive(Default)]
struct DeepSeekAccumulator {
    inner: OpenAiAccumulator,
}

impl ProviderResponseAccumulator for DeepSeekAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>, InferenceError> {
        self.inner.capture_reasoning = true;
        self.inner.push(chunk)
    }

    fn finish(
        mut self: Box<Self>,
    ) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>), InferenceError> {
        self.inner.capture_reasoning = true;
        Box::new(self.inner).finish()
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
    use margatroid_types::{Message, TokenUsage, ToolCall, ToolDefinition};
    use serde_json::json;

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
        let mut accumulator = DeepSeekAccumulator::default();
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

        let (response, trailing) = Box::new(accumulator).finish().unwrap();
        assert_eq!(trailing, [ProviderStreamDelta::Content("tail".into())]);
        assert_eq!(response.content, Some("tail".into()));
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn provider_tools_keep_agent_local_names() {
        let tools = vec![
            ToolDefinition {
                name: "skill0_review".into(),
                description: "Review code.".into(),
                input_schema: json!({"type":"object"}),
            },
            ToolDefinition {
                name: "skill1_commit".into(),
                description: "Commit code.".into(),
                input_schema: json!({"type":"object"}),
            },
        ];
        let request = OpenAiRequest::from_input(ProviderInput::new(
            "model",
            &InferenceParameters::default(),
            &[Message::User {
                content: "hello".into(),
            }],
            &tools,
        ));
        assert_eq!(request.tools[0]["function"]["name"], "skill0_review");
        assert_eq!(request.tools[1]["function"]["name"], "skill1_commit");
        assert!(
            serde_json::to_value(request).unwrap()["stream_options"]["include_usage"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn deepseek_request_only_returns_tool_call_reasoning_to_the_provider() {
        let messages = vec![
            Message::Assistant {
                reasoning: Some("ordinary reasoning".into()),
                content: Some("ordinary answer".into()),
                tool_calls: Vec::new(),
            },
            Message::Assistant {
                reasoning: Some("tool reasoning".into()),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    tool_name: "tool0".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Assistant {
                reasoning: None,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-2".into(),
                    tool_name: "tool0".into(),
                    arguments: "{}".into(),
                }],
            },
        ];
        let request = OpenAiRequest::from_deepseek_input(
            ProviderInput::new("model", &InferenceParameters::default(), &messages, &[]),
            true,
            Some("high".into()),
        );
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(value["thinking"]["type"], "enabled");
        assert_eq!(value["stream_options"]["include_usage"], true);
        assert_eq!(value["reasoning_effort"], "high");
        assert!(value["messages"][0].get("reasoning_content").is_none());
        assert!(value["messages"][0].get("tool_calls").is_none());
        assert_eq!(value["messages"][1]["reasoning_content"], "tool reasoning");
        assert_eq!(
            value["messages"][1]["tool_calls"].as_array().unwrap().len(),
            1
        );
        assert_eq!(value["messages"][2]["reasoning_content"], "");
    }

    #[test]
    fn openai_assistant_omits_empty_tool_calls() {
        let without_tools = openai_message(&Message::Assistant {
            reasoning: None,
            content: Some("answer".into()),
            tool_calls: Vec::new(),
        });
        let with_tools = openai_message(&Message::Assistant {
            reasoning: None,
            content: None,
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                tool_name: "tool0".into(),
                arguments: "{}".into(),
            }],
        });

        assert!(without_tools.get("tool_calls").is_none());
        assert_eq!(with_tools["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn deepseek_accumulator_separates_reasoning_and_content() {
        let mut accumulator = DeepSeekAccumulator::default();
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
