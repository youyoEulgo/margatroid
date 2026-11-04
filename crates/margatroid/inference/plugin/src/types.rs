use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(test)]
use app_runtime_plugin::RuntimeEventSender;
use client_core::ClientSender;
use core_plugin::{Entity, Event, Resource};
use inference_core::{
    DomainRequest, InferenceError, InferenceErrorKind, InferenceImage, InferenceMessage,
    InferenceRequest, InferenceResponse, InferenceStopReason, InferenceToolCall, InferenceUsage,
    ModelRoute,
};
use lua_runtime_core::DriverLuaVmInstance;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

type Result<T> = std::result::Result<T, InferenceError>;

pub(crate) const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_MESSAGES_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const FRAGMENT_CAPACITY: usize = 32;

pub(crate) struct HttpRuntime {
    pub client: reqwest::Client,
    pub schedule: String,
}
impl Resource for HttpRuntime {}

#[derive(Clone)]
pub(crate) struct InferenceCleanupFinished {
    pub agent: Entity,
    pub request_id: String,
    pub instance: DriverLuaVmInstance,
}
impl Event for InferenceCleanupFinished {}

pub(crate) struct InferenceContext {
    pub request: InferenceRequest,
    pub reply: Option<oneshot::Sender<Result<InferenceResponse>>>,
    pub route: ModelRoute,
    pub tools: Vec<(String, PathBuf)>,
    pub client: reqwest::Client,
    pub gate: Arc<Mutex<StreamGate>>,
    pub delivery: StreamDelivery,
    pub cleanup_notification: Option<(InferenceCleanupSender, InferenceCleanupFinished)>,
    #[cfg(test)]
    pub cleanup_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

#[derive(Clone)]
pub(crate) struct StreamDelivery {
    pub id: String,
    pub agent: String,
    pub instance: String,
    pub senders: Vec<ClientSender>,
}

#[derive(Default)]
pub(crate) struct StreamGate {
    pub closed: bool,
    pub sequence: u64,
    pub accumulator: ResponseAccumulator,
}
impl StreamGate {
    pub fn close(&mut self) {
        self.closed = true;
    }
}

pub(crate) enum ProviderFragment {
    Content(String),
    Reasoning(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    Replay(Value),
    Usage(InferenceUsage),
    Finished(InferenceStopReason),
}

#[derive(Default)]
pub(crate) struct ResponseAccumulator {
    pub content: String,
    pub reasoning: String,
    pub calls: BTreeMap<usize, CallBuilder>,
    pub replay: BTreeMap<usize, Value>,
    pub replay_format: Option<String>,
    pub usage: Option<InferenceUsage>,
    pub reason: Option<InferenceStopReason>,
    pub bytes: usize,
}

#[derive(Default)]
pub(crate) struct CallBuilder {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub(crate) struct FormatDecoder {
    pub format: ApiFormat,
    pub buffer: Vec<u8>,
    pub data: Vec<u8>,
    pub event: String,
    pub bytes: usize,
    pub started: bool,
    pub terminal: bool,
    pub reason: Option<InferenceStopReason>,
    pub blocks: BTreeMap<usize, BlockState>,
    pub usage: InferenceUsage,
    pub refused: bool,
}

#[derive(Default)]
pub(crate) struct BlockState {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub closed: bool,
    pub text: Sha256,
    pub reasoning: Sha256,
    pub arguments: Sha256,
    pub has_arguments: bool,
    pub encrypted: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelRouteDocument {
    pub models: Vec<ModelRouteConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelRouteConfig {
    pub id: String,
    pub model: String,
    pub provider: Vendor,
    pub api_type: ApiFormat,
    pub base_url: String,
    pub api_key: String,
    pub context_window: Option<String>,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}

pub(crate) struct ProviderRouteInput<'a> {
    pub base_url: &'a Url,
    pub api_key: &'a str,
    pub thinking: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
}

pub(crate) struct ProviderHttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub decoder: FormatDecoder,
}
impl ProviderHttpRequest {
    fn new(
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        decoder: FormatDecoder,
    ) -> Self {
        Self {
            method,
            url,
            headers,
            body,
            decoder,
        }
    }
}

fn invalid(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::InvalidParameters, message)
}
fn decode(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::ResponseInvalid, message)
}
fn incomplete(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::ResponseIncomplete, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApiFormat {
    ChatCompletions,
    Messages,
    Responses,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ApiRequest {
    ChatCompletions(ChatCompletionsRequest),
    Messages(MessagesRequest),
    Responses(ResponsesRequest),
}

impl ApiFormat {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::Responses => "responses",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatCompletionsRequest {
    pub(crate) model: String,
    pub(crate) messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<Value>,
    pub(crate) stream: bool,
    pub(crate) stream_options: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) stop: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MessagesRequest {
    pub(crate) model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) system: Vec<Value>,
    pub(crate) messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<Value>,
    pub(crate) stream: bool,
    pub(crate) max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) stop_sequences: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResponsesRequest {
    pub(crate) model: String,
    pub(crate) input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<Value>,
    pub(crate) stream: bool,
    pub(crate) store: bool,
    pub(crate) include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) top_p: Option<f32>,
}

pub(crate) fn validated_message_sizes(message: &InferenceMessage) -> Result<(usize, usize)> {
    if let InferenceMessage::User { images, .. } | InferenceMessage::Tool { images, .. } = message {
        validate_user_images(images)?;
    }
    let encoded = serde_json::to_vec(message)
        .map_err(|_| invalid("invalid message"))?
        .len();
    let text = match message {
        InferenceMessage::User { content, images } if !images.is_empty() => {
            serde_json::to_vec(&InferenceMessage::User {
                content: content.clone(),
                images: vec![],
            })
            .map_err(|_| invalid("invalid message"))?
            .len()
        }
        InferenceMessage::Tool { images, .. } if !images.is_empty() => {
            let mut text_only = message.clone();
            if let InferenceMessage::Tool { images, .. } = &mut text_only {
                images.clear();
            }
            serde_json::to_vec(&text_only)
                .map_err(|_| invalid("invalid message"))?
                .len()
        }
        _ => encoded,
    };
    Ok((text, encoded))
}

fn user_content(content: String, images: Vec<InferenceImage>, format: ApiFormat) -> Value {
    if images.is_empty() && format != ApiFormat::Messages {
        return json!(content);
    }
    let mut parts = Vec::new();
    if !content.is_empty() || images.is_empty() {
        let kind = if format == ApiFormat::Responses {
            "input_text"
        } else {
            "text"
        };
        parts.push(json!({"type":kind,"text":content}));
    }
    for image in images {
        parts.push(match format {
            ApiFormat::ChatCompletions => json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",image.media_type,image.data)}}),
            ApiFormat::Messages => json!({"type":"image","source":{"type":"base64","media_type":image.media_type,"data":image.data}}),
            ApiFormat::Responses => json!({"type":"input_image","image_url":format!("data:{};base64,{}",image.media_type,image.data)}),
        });
    }
    json!(parts)
}

fn replay_items(replay: Option<Value>, format: ApiFormat) -> Result<Vec<Value>> {
    let Some(replay) = replay else {
        return Ok(Vec::new());
    };
    let source: ApiFormat = serde_json::from_value(replay["api_format"].clone())
        .map_err(|_| invalid("unknown historical reasoning replay API format"))?;
    let items = strict_replay_items(&replay, source)?;
    if source != format {
        return Ok(Vec::new());
    }
    Ok(items)
}

fn strict_replay_items(replay: &Value, format: ApiFormat) -> Result<Vec<Value>> {
    if replay["api_format"] != format.as_str() {
        return Err(invalid("reasoning replay API format mismatch"));
    }
    let items = replay["items"]
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid("invalid reasoning replay items"))?;
    for item in items {
        let valid = match format {
            ApiFormat::Messages => match item["type"].as_str() {
                Some("thinking") => {
                    item["thinking"].is_string()
                        && item["signature"].as_str().is_some_and(|s| !s.is_empty())
                }
                Some("redacted_thinking") => item["data"].as_str().is_some_and(|s| !s.is_empty()),
                _ => false,
            },
            ApiFormat::Responses => {
                item["type"] == "reasoning"
                    && item["id"].as_str().is_some_and(|s| !s.is_empty())
                    && item["summary"].is_array()
                    && item.get("content").is_none_or(Value::is_array)
                    && item
                        .get("encrypted_content")
                        .is_none_or(|v| v.is_null() || v.is_string())
            }
            ApiFormat::ChatCompletions => false,
        };
        if !valid {
            return Err(invalid("invalid reasoning replay item"));
        }
    }
    Ok(items.clone())
}

#[derive(Clone, Copy)]
pub(crate) enum ChatReasoning {
    Omit,
    Preserve,
    DeepSeek { disabled: bool },
}

pub(crate) struct EncodeInput {
    pub request: DomainRequest,
    pub chat_reasoning: ChatReasoning,
}

impl TryFrom<DomainRequest> for ChatCompletionsRequest {
    type Error = InferenceError;
    fn try_from(request: DomainRequest) -> Result<Self> {
        EncodeInput {
            request,
            chat_reasoning: ChatReasoning::Preserve,
        }
        .try_into()
    }
}

impl TryFrom<EncodeInput> for ChatCompletionsRequest {
    type Error = InferenceError;
    fn try_from(input: EncodeInput) -> Result<Self> {
        let EncodeInput {
            mut request,
            chat_reasoning,
        } = input;
        normalize(&mut request)?;
        let mut messages = Vec::new();
        let mut pending = HashSet::new();
        let mut image_blocks = Vec::new();
        for message in request.messages {
            let value = match message {
                InferenceMessage::System { content } => json!({"role":"system","content":content}),
                InferenceMessage::User { content, images } => {
                    json!({"role":"user","content":user_content(content, images, ApiFormat::ChatCompletions)})
                }
                InferenceMessage::Assistant {
                    reasoning,
                    replay,
                    content,
                    tool_calls,
                    ..
                } => {
                    replay_items(replay, ApiFormat::ChatCompletions)?;
                    if matches!(chat_reasoning, ChatReasoning::Omit)
                        && content.as_ref().is_none_or(String::is_empty)
                        && tool_calls.is_empty()
                    {
                        continue;
                    }
                    pending.extend(tool_calls.iter().map(|call| call.id.clone()));
                    let mut value = json!({"role":"assistant","content":content});
                    match chat_reasoning {
                        ChatReasoning::Omit => {}
                        ChatReasoning::Preserve => {
                            if let Some(reasoning) = reasoning {
                                value["reasoning_content"] = json!(reasoning);
                            }
                        }
                        ChatReasoning::DeepSeek { disabled } => {
                            if !disabled || !tool_calls.is_empty() {
                                value["reasoning_content"] = json!(reasoning.unwrap_or_default());
                            }
                        }
                    }
                    if !tool_calls.is_empty() {
                        value["tool_calls"] = json!(tool_calls.into_iter().map(|call| json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments.to_string()}})).collect::<Vec<_>>());
                    }
                    value
                }
                InferenceMessage::Tool {
                    call_id,
                    content,
                    images,
                    ..
                } => {
                    pending.remove(&call_id);
                    for image in images {
                        image_blocks
                            .push(json!({"type":"text","text":format!("tool_call_id: {call_id}")}));
                        image_blocks.extend(
                            user_content(String::new(), vec![image], ApiFormat::ChatCompletions)
                                .as_array()
                                .unwrap()
                                .iter()
                                .cloned(),
                        );
                    }
                    json!({"role":"tool","tool_call_id":call_id,"content":content})
                }
            };
            messages.push(value);
            if pending.is_empty() && !image_blocks.is_empty() {
                messages.push(json!({"role":"user","content":std::mem::take(&mut image_blocks)}));
            }
        }
        Ok(Self { model: request.config.model, messages, tools: request.tools.into_iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.input_schema}})).collect(), stream:true, stream_options:json!({"include_usage":true}), temperature:request.config.temperature, max_tokens:request.config.max_output_tokens, top_p:request.config.top_p, stop:request.config.stop })
    }
}

fn merge_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = messages.last_mut().filter(|last| last["role"] == role) {
        last["content"].as_array_mut().unwrap().extend(blocks);
    } else {
        messages.push(json!({"role":role,"content":blocks}));
    }
}

impl TryFrom<DomainRequest> for MessagesRequest {
    type Error = InferenceError;
    fn try_from(mut request: DomainRequest) -> Result<Self> {
        normalize(&mut request)?;
        let max_tokens = request
            .config
            .max_output_tokens
            .ok_or_else(|| invalid("Messages requires max_output_tokens"))?;
        if request.config.temperature.is_some_and(|t| t > 1.0) {
            return Err(invalid("Messages temperature must be at most one"));
        }
        let mut system = Vec::new();
        let mut messages = Vec::new();
        for message in request.messages {
            match message {
                InferenceMessage::System { content } => {
                    if !messages.is_empty() {
                        return Err(invalid(
                            "Messages cannot preserve interleaved system messages",
                        ));
                    }
                    system.push(json!({"type":"text","text":content}));
                }
                InferenceMessage::User { content, images } => merge_message(
                    &mut messages,
                    "user",
                    user_content(content, images, ApiFormat::Messages)
                        .as_array()
                        .unwrap()
                        .clone(),
                ),
                InferenceMessage::Assistant {
                    replay,
                    content,
                    tool_calls,
                    ..
                } => {
                    let mut blocks = replay_items(replay, ApiFormat::Messages)?;
                    if let Some(content) = content {
                        blocks.push(json!({"type":"text","text":content}));
                    }
                    for call in tool_calls {
                        blocks.push(json!({"type":"tool_use","id":call.id,"name":call.name,"input":call.arguments}));
                    }
                    if blocks.is_empty() {
                        continue;
                    }
                    merge_message(&mut messages, "assistant", blocks);
                }
                InferenceMessage::Tool {
                    call_id,
                    content,
                    images,
                    failed,
                    ..
                } => merge_message(
                    &mut messages,
                    "user",
                    vec![
                        json!({"type":"tool_result","tool_use_id":call_id,"content":if images.is_empty() { json!(content) } else { user_content(content, images, ApiFormat::Messages) },"is_error":failed}),
                    ],
                ),
            }
        }
        Ok(Self { model:request.config.model,system,messages,tools:request.tools.into_iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.input_schema})).collect(),stream:true,max_tokens,temperature:request.config.temperature,top_p:request.config.top_p,stop_sequences:request.config.stop })
    }
}

impl TryFrom<DomainRequest> for ResponsesRequest {
    type Error = InferenceError;
    fn try_from(mut request: DomainRequest) -> Result<Self> {
        normalize(&mut request)?;
        if !request.config.stop.is_empty() {
            return Err(invalid("Responses cannot encode stop sequences"));
        }
        let mut input = Vec::new();
        for message in request.messages {
            match message {
                InferenceMessage::System { content } => {
                    input.push(json!({"role":"system","content":content}))
                }
                InferenceMessage::User { content, images } => {
                    input.push(json!({"role":"user","content":user_content(content, images, ApiFormat::Responses)}))
                }
                InferenceMessage::Assistant {
                    replay,
                    content,
                    tool_calls,
                    ..
                } => {
                    input.extend(replay_items(replay, ApiFormat::Responses)?);
                    if let Some(content) = content {
                        input.push(json!({"role":"assistant","content":[{"type":"output_text","text":content}]}));
                    }
                    for call in tool_calls {
                        input.push(json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments.to_string()}));
                    }
                }
                InferenceMessage::Tool {
                    call_id,
                    content,
                    images,
                    ..
                } => input.push(
                    json!({"type":"function_call_output","call_id":call_id,"output":user_content(content, images, ApiFormat::Responses)}),
                ),
            }
        }
        Ok(Self { model:request.config.model,input,tools:request.tools.into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.input_schema,"strict":false})).collect(),stream:true,store:false,include:vec!["reasoning.encrypted_content".into()],temperature:request.config.temperature,max_output_tokens:request.config.max_output_tokens,top_p:request.config.top_p })
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Vendor {
    OpenAi,
    Anthropic,
    DeepSeek,
    OpenRouter,
}

impl Vendor {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }

    pub(crate) fn supports(self, format: ApiFormat) -> bool {
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

pub(crate) struct Transport {
    base_url: Url,
    api_key: String,
}

#[derive(Default, Serialize)]
struct DeepSeekOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct ThinkingOption {
    r#type: &'static str,
}

#[derive(Default, Serialize)]
struct OpenRouterOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<RouterReasoning>,
}

#[derive(Serialize)]
struct RouterReasoning {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Serialize)]
struct VendorRequest<'a, T, O> {
    #[serde(flatten)]
    request: &'a T,
    #[serde(flatten)]
    options: &'a O,
}

pub(crate) struct OpenAi<T> {
    transport: Transport,
    request: std::marker::PhantomData<T>,
}

pub(crate) struct Anthropic<T> {
    transport: Transport,
    request: std::marker::PhantomData<T>,
}

pub(crate) struct DeepSeek<T> {
    transport: Transport,
    options: DeepSeekOptions,
    request: std::marker::PhantomData<T>,
}

pub(crate) struct OpenRouter<T> {
    transport: Transport,
    options: OpenRouterOptions,
    request: std::marker::PhantomData<T>,
}

pub(crate) enum ApiAdapter {
    OpenAiChat(OpenAi<ChatCompletionsRequest>),
    OpenAiResponses(OpenAi<ResponsesRequest>),
    AnthropicMessages(Anthropic<MessagesRequest>),
    DeepSeekChat(DeepSeek<ChatCompletionsRequest>),
    DeepSeekMessages(DeepSeek<MessagesRequest>),
    DeepSeekResponses(DeepSeek<ResponsesRequest>),
    OpenRouterChat(OpenRouter<ChatCompletionsRequest>),
}

pub(crate) fn build_adapter(
    vendor: Vendor,
    format: ApiFormat,
    route: ProviderRouteInput<'_>,
) -> Result<ApiAdapter> {
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
        return Err(invalid("thinking/reasoning_effort options are supported only for DeepSeek/OpenRouter chat_completions"));
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
        (Some(ThinkingMode::Enabled), None) => return Err(invalid("enabled thinking requires reasoning_effort")),
        (_, None) => None,
        _ => return Err(invalid("reasoning_effort requires enabled thinking: DeepSeek supports high/max; OpenRouter supports high")),
    };
    let transport = Transport {
        base_url: route.base_url.clone(),
        api_key: route.api_key.to_owned(),
    };
    let request = std::marker::PhantomData;
    Ok(match (vendor, format) {
        (Vendor::OpenAi, ApiFormat::ChatCompletions) => {
            ApiAdapter::OpenAiChat(OpenAi { transport, request })
        }
        (Vendor::OpenAi, ApiFormat::Responses) => ApiAdapter::OpenAiResponses(OpenAi {
            transport,
            request: std::marker::PhantomData,
        }),
        (Vendor::Anthropic, ApiFormat::Messages) => ApiAdapter::AnthropicMessages(Anthropic {
            transport,
            request: std::marker::PhantomData,
        }),
        (Vendor::DeepSeek, ApiFormat::ChatCompletions) => ApiAdapter::DeepSeekChat(DeepSeek {
            transport,
            options: DeepSeekOptions {
                thinking: thinking.map(|mode| ThinkingOption {
                    r#type: mode.as_str(),
                }),
                reasoning_effort,
            },
            request,
        }),
        (Vendor::DeepSeek, ApiFormat::Messages) => ApiAdapter::DeepSeekMessages(DeepSeek {
            transport,
            options: DeepSeekOptions::default(),
            request: std::marker::PhantomData,
        }),
        (Vendor::DeepSeek, ApiFormat::Responses) => ApiAdapter::DeepSeekResponses(DeepSeek {
            transport,
            options: DeepSeekOptions::default(),
            request: std::marker::PhantomData,
        }),
        (Vendor::OpenRouter, ApiFormat::ChatCompletions) => {
            ApiAdapter::OpenRouterChat(OpenRouter {
                transport,
                options: OpenRouterOptions {
                    reasoning: thinking.map(|mode| RouterReasoning {
                        enabled: mode == ThinkingMode::Enabled,
                        effort: reasoning_effort,
                    }),
                },
                request,
            })
        }
        _ => return Err(invalid("provider does not support the selected API format")),
    })
}

impl ApiAdapter {
    pub(crate) fn encode(&self, request: DomainRequest) -> Result<ApiRequest> {
        match self {
            Self::OpenAiChat(_) => Ok(ApiRequest::ChatCompletions(
                EncodeInput {
                    request,
                    chat_reasoning: ChatReasoning::Omit,
                }
                .try_into()?,
            )),
            Self::DeepSeekChat(handler) => Ok(ApiRequest::ChatCompletions(
                EncodeInput {
                    request,
                    chat_reasoning: ChatReasoning::DeepSeek {
                        disabled: handler
                            .options
                            .thinking
                            .as_ref()
                            .is_some_and(|option| option.r#type == "disabled"),
                    },
                }
                .try_into()?,
            )),
            Self::OpenRouterChat(_) => Ok(ApiRequest::ChatCompletions(
                EncodeInput {
                    request,
                    chat_reasoning: ChatReasoning::Preserve,
                }
                .try_into()?,
            )),
            Self::AnthropicMessages(_) | Self::DeepSeekMessages(_) => {
                Ok(ApiRequest::Messages(request.try_into()?))
            }
            Self::OpenAiResponses(_) | Self::DeepSeekResponses(_) => {
                Ok(ApiRequest::Responses(request.try_into()?))
            }
        }
    }

    pub(crate) fn process_request(&self, request: ApiRequest) -> Result<ProviderHttpRequest> {
        match (self, request) {
            (Self::OpenAiChat(handler), ApiRequest::ChatCompletions(request)) => {
                handler.process(request)
            }
            (Self::OpenAiResponses(handler), ApiRequest::Responses(request)) => {
                handler.process(request)
            }
            (Self::AnthropicMessages(handler), ApiRequest::Messages(request)) => {
                handler.process(request)
            }
            (Self::DeepSeekChat(handler), ApiRequest::ChatCompletions(request)) => {
                handler.process(request)
            }
            (Self::DeepSeekMessages(handler), ApiRequest::Messages(request)) => {
                handler.process(request)
            }
            (Self::DeepSeekResponses(handler), ApiRequest::Responses(request)) => {
                handler.process(request)
            }
            (Self::OpenRouterChat(handler), ApiRequest::ChatCompletions(request)) => {
                handler.process(request)
            }
            _ => Err(invalid(
                "request format does not match the selected API adapter",
            )),
        }
    }
}

impl OpenAi<ChatCompletionsRequest> {
    fn process(&self, request: ChatCompletionsRequest) -> Result<ProviderHttpRequest> {
        self.transport.prepare(
            &request,
            "/chat/completions",
            false,
            ApiFormat::ChatCompletions,
        )
    }
}

impl OpenAi<ResponsesRequest> {
    fn process(&self, request: ResponsesRequest) -> Result<ProviderHttpRequest> {
        self.transport
            .prepare(&request, "/responses", false, ApiFormat::Responses)
    }
}

impl Anthropic<MessagesRequest> {
    fn process(&self, request: MessagesRequest) -> Result<ProviderHttpRequest> {
        self.transport
            .prepare(&request, "/messages", true, ApiFormat::Messages)
    }
}

impl DeepSeek<ChatCompletionsRequest> {
    fn process(&self, request: ChatCompletionsRequest) -> Result<ProviderHttpRequest> {
        self.transport.prepare(
            &VendorRequest {
                request: &request,
                options: &self.options,
            },
            "/chat/completions",
            false,
            ApiFormat::ChatCompletions,
        )
    }
}

impl DeepSeek<MessagesRequest> {
    fn process(&self, request: MessagesRequest) -> Result<ProviderHttpRequest> {
        self.transport
            .prepare(&request, "/messages", true, ApiFormat::Messages)
    }
}

impl DeepSeek<ResponsesRequest> {
    fn process(&self, request: ResponsesRequest) -> Result<ProviderHttpRequest> {
        self.transport
            .prepare(&request, "/responses", false, ApiFormat::Responses)
    }
}

impl OpenRouter<ChatCompletionsRequest> {
    fn process(&self, request: ChatCompletionsRequest) -> Result<ProviderHttpRequest> {
        self.transport.prepare(
            &VendorRequest {
                request: &request,
                options: &self.options,
            },
            "/chat/completions",
            false,
            ApiFormat::ChatCompletions,
        )
    }
}

impl Transport {
    fn prepare<T: Serialize>(
        &self,
        request: &T,
        suffix: &str,
        messages_auth: bool,
        format: ApiFormat,
    ) -> Result<ProviderHttpRequest> {
        let url = Url::parse(&format!(
            "{}{}",
            self.base_url.as_str().trim_end_matches('/'),
            suffix
        ))
        .map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::InvalidRequest,
                "provider endpoint is invalid",
            )
        })?;
        let mut headers = HeaderMap::new();
        let header_error = |_| {
            InferenceError::new(
                InferenceErrorKind::InvalidRequest,
                "provider authorization header is invalid",
            )
        };
        if messages_auth {
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
        let body = serde_json::to_vec(request).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::InvalidRequest,
                "provider request could not be encoded",
            )
        })?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "encoded provider request exceeds 32 MiB",
            ));
        }
        Ok(ProviderHttpRequest::new(
            Method::POST,
            url,
            headers,
            body,
            FormatDecoder::new(format),
        ))
    }
}

pub(crate) fn normalize(request: &mut DomainRequest) -> Result<()> {
    let parameters = &request.config;
    if parameters.model.trim().is_empty()
        || parameters.model.len() > 256
        || parameters.model.chars().any(char::is_control)
        || parameters
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        || parameters
            .top_p
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        || parameters.max_output_tokens == Some(0)
        || parameters.stop.len() > 64
        || parameters
            .stop
            .iter()
            .any(|value| value.is_empty() || value.len() > 256)
    {
        return Err(invalid("invalid inference parameters"));
    }
    if request.messages.is_empty() || request.messages.len() > 4096 {
        return Err(invalid("message count outside limits"));
    }
    if request.tools.len() > 256 {
        return Err(invalid("too many tools"));
    }
    let mut names = HashSet::new();
    for tool in &request.tools {
        if tool.name.is_empty()
            || tool.name.len() > 64
            || !tool
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !names.insert(&tool.name)
            || !tool.input_schema.is_object()
            || tool.description.trim().is_empty()
            || tool.description.len() > 8192
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidToolDefinition,
                "invalid tool definition",
            ));
        }
    }
    let mut pending = HashSet::new();
    let mut seen = HashSet::new();
    let mut text_bytes = 0usize;
    let mut encoded_bytes = 0usize;
    for message in &request.messages {
        let (text, encoded) = validated_message_sizes(message)?;
        text_bytes = text_bytes.saturating_add(text);
        encoded_bytes = encoded_bytes.saturating_add(encoded);
        if text > MAX_MESSAGE_BYTES
            || text_bytes > MAX_MESSAGES_BYTES
            || encoded_bytes > MAX_REQUEST_BYTES
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "message bytes exceed budget",
            ));
        }
        match message {
            InferenceMessage::Assistant {
                content,
                reasoning,
                tool_calls,
                replay,
            } => {
                if !pending.is_empty()
                    || (content.as_ref().is_none_or(String::is_empty)
                        && reasoning.as_ref().is_none_or(String::is_empty)
                        && tool_calls.is_empty()
                        && replay.is_none())
                {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "invalid assistant history",
                    ));
                }
                if tool_calls.len() > 256 {
                    return Err(invalid("too many tool calls"));
                }
                for call in tool_calls {
                    if call.id.is_empty()
                        || call.id.len() > 256
                        || call.name.is_empty()
                        || call.name.len() > 64
                        || !seen.insert(call.id.clone())
                        || !call.arguments.is_object()
                    {
                        return Err(InferenceError::new(
                            InferenceErrorKind::InvalidMessages,
                            "invalid tool call history",
                        ));
                    }
                    pending.insert(call.id.clone());
                }
            }
            InferenceMessage::Tool { call_id, .. } => {
                if !pending.remove(call_id) {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "tool result has no matching pending call",
                    ));
                }
            }
            _ if !pending.is_empty() => {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "tool result batch interrupted",
                ))
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidMessages,
            "tool results missing",
        ));
    }
    Ok(())
}

fn validate_user_images(images: &[InferenceImage]) -> Result<()> {
    use base64::Engine;
    if images.len() > 8 {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidMessages,
            "too many images",
        ));
    }
    let mut total = 0usize;
    for image in images {
        if image.data.is_empty()
            || image.data.len() > (5 * 1024 * 1024usize).div_ceil(3) * 4
            || !image.data.len().is_multiple_of(4)
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "invalid image size or base64 length",
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| {
                InferenceError::new(InferenceErrorKind::InvalidMessages, "invalid image base64")
            })?;
        total = total.saturating_add(bytes.len());
        if bytes.len() > 5 * 1024 * 1024 || total > 20 * 1024 * 1024 {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "decoded images exceed budget",
            ));
        }
        let valid = match image.media_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            _ => false,
        };
        if !valid {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "image media type and signature disagree",
            ));
        }
    }
    Ok(())
}

pub(crate) fn build_request(
    input: DomainRequest,
    route: &ModelRoute,
) -> Result<ProviderHttpRequest> {
    let base_url = Url::parse(&route.base_url).map_err(|_| {
        InferenceError::new(InferenceErrorKind::InvalidModelRoute, "invalid base URL")
    })?;
    let vendor = serde_json::from_value::<Vendor>(json!(route.provider)).map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::UnsupportedFormat,
            "unsupported provider",
        )
    })?;
    let format = serde_json::from_value::<ApiFormat>(json!(route.api_type)).map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::UnsupportedFormat,
            "unsupported API format",
        )
    })?;
    let adapter = build_adapter(
        vendor,
        format,
        ProviderRouteInput {
            base_url: &base_url,
            api_key: &route.api_key,
            thinking: route.thinking.as_deref(),
            reasoning_effort: route.reasoning_effort.as_deref(),
        },
    )?;
    adapter.process_request(adapter.encode(input)?)
}

impl ResponseAccumulator {
    pub fn accept(&mut self, fragment: ProviderFragment) -> Result<Option<(bool, String)>> {
        let size = match &fragment {
            ProviderFragment::Content(value) | ProviderFragment::Reasoning(value) => value.len(),
            ProviderFragment::ToolCallDelta {
                id,
                name,
                arguments,
                ..
            } => id
                .as_ref()
                .map_or(0, String::len)
                .saturating_add(name.as_ref().map_or(0, String::len))
                .saturating_add(arguments.len()),
            ProviderFragment::Replay(value) => serde_json::to_vec(value)
                .map_err(|_| decode("invalid replay value"))?
                .len(),
            _ => 0,
        };
        self.bytes = self
            .bytes
            .checked_add(size)
            .filter(|&bytes| bytes <= MAX_REQUEST_BYTES)
            .ok_or_else(|| {
                InferenceError::new(
                    InferenceErrorKind::ResponseTooLarge,
                    "accepted response exceeds budget",
                )
            })?;
        match fragment {
            ProviderFragment::Content(value) => {
                self.content.push_str(&value);
                Ok(Some((false, value)))
            }
            ProviderFragment::Reasoning(value) => {
                self.reasoning.push_str(&value);
                Ok(Some((true, value)))
            }
            ProviderFragment::ToolCallDelta {
                index,
                id,
                name,
                arguments,
            } => {
                if index >= 256 {
                    return Err(decode("tool call index exceeds limit"));
                }
                let call = self.calls.entry(index).or_default();
                if let Some(id) = id {
                    call.id.push_str(&id);
                }
                if let Some(name) = name {
                    call.name.push_str(&name);
                }
                call.arguments.push_str(&arguments);
                if call.id.len() > 256
                    || call.name.len() > 64
                    || call.arguments.len() > MAX_MESSAGE_BYTES
                {
                    return Err(InferenceError::new(
                        InferenceErrorKind::ResponseTooLarge,
                        "tool call exceeds budget",
                    ));
                }
                Ok(None)
            }
            ProviderFragment::Replay(value) => {
                let format = value["api_format"]
                    .as_str()
                    .ok_or_else(|| decode("replay format missing"))?;
                if self
                    .replay_format
                    .as_deref()
                    .is_some_and(|current| current != format)
                {
                    return Err(decode("replay format changed"));
                }
                self.replay_format = Some(format.to_owned());
                let index = event_index(&value, "index")?;
                match value["op"].as_str() {
                    Some("start") | Some("replace") => {
                        self.replay.insert(index, value["item"].clone());
                    }
                    Some("thinking_delta") | Some("signature_delta") => {
                        let field = if value["op"] == "thinking_delta" {
                            "thinking"
                        } else {
                            "signature"
                        };
                        let item = self
                            .replay
                            .get_mut(&index)
                            .ok_or_else(|| decode("replay delta before start"))?;
                        let mut text = item[field]
                            .as_str()
                            .ok_or_else(|| decode("invalid replay text"))?
                            .to_owned();
                        text.push_str(event_string(&value, "value")?);
                        item[field] = json!(text);
                    }
                    _ => return Err(decode("unknown replay operation")),
                }
                Ok(None)
            }
            ProviderFragment::Usage(usage) => {
                self.usage = Some(usage);
                Ok(None)
            }
            ProviderFragment::Finished(reason) => {
                if self.reason.replace(reason).is_some() {
                    return Err(decode("duplicate accepted terminal"));
                }
                Ok(None)
            }
        }
    }

    pub fn response(&self, tools: &HashSet<String>) -> Result<InferenceResponse> {
        let reason = self
            .reason
            .clone()
            .ok_or_else(|| incomplete("stream lacks accepted terminal"))?;
        let mut seen = HashSet::new();
        let calls = self
            .calls
            .values()
            .map(|call| {
                let arguments = serde_json::from_str::<Value>(&call.arguments)
                    .map_err(|_| incomplete("incomplete tool arguments"))?;
                if call.id.is_empty()
                    || call.name.is_empty()
                    || !tools.contains(&call.name)
                    || !seen.insert(call.id.clone())
                    || !arguments.is_object()
                {
                    return Err(incomplete("invalid, duplicate or unknown tool call"));
                }
                Ok(InferenceToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if reason == InferenceStopReason::ToolCalls && calls.is_empty() {
            return Err(incomplete("tool terminal has no calls"));
        }
        if self.content.is_empty()
            && calls.is_empty()
            && !matches!(
                reason,
                InferenceStopReason::Length | InferenceStopReason::ContentFilter
            )
        {
            return Err(incomplete("response has no content or tool calls"));
        }
        let replay = if self.replay.is_empty() {
            None
        } else {
            Some(
                json!({"api_format":self.replay_format,"items":self.replay.values().collect::<Vec<_>>()}),
            )
        };
        if let Some(format) = &self.replay_format {
            let format = match format.as_str() {
                "messages" => ApiFormat::Messages,
                "responses" => ApiFormat::Responses,
                _ => return Err(decode("invalid replay format")),
            };
            strict_replay_items(
                replay
                    .as_ref()
                    .ok_or_else(|| decode("response reasoning replay is missing"))?,
                format,
            )
            .map_err(|_| decode("response reasoning replay is incomplete"))?;
        }
        Ok(InferenceResponse {
            content: (!self.content.is_empty()).then(|| self.content.clone()),
            reasoning: (!self.reasoning.is_empty()).then(|| self.reasoning.clone()),
            tool_calls: calls,
            replay,
            usage: self.usage.clone(),
            stop_reason: reason,
        })
    }
}

fn event_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| decode("missing event string field"))
}

fn event_index(value: &Value, key: &str) -> Result<usize> {
    value[key]
        .as_u64()
        .filter(|&index| index < 256)
        .map(|index| index as usize)
        .ok_or_else(|| decode("missing or invalid event index"))
}

fn hash_matches(hash: &Sha256, text: &str) -> bool {
    hash.clone().finalize() == Sha256::digest(text.as_bytes())
}

fn stop_reason(value: &str) -> InferenceStopReason {
    match value {
        "stop" | "end_turn" | "stop_sequence" => InferenceStopReason::Completed,
        "tool_calls" | "tool_use" => InferenceStopReason::ToolCalls,
        "length" | "max_tokens" | "max_output_tokens" | "model_context_window_exceeded" => {
            InferenceStopReason::Length
        }
        "content_filter" | "refusal" => InferenceStopReason::ContentFilter,
        other => InferenceStopReason::Other(other.to_owned()),
    }
}

impl FormatDecoder {
    pub fn new(format: ApiFormat) -> Self {
        Self {
            format,
            buffer: vec![],
            data: vec![],
            event: String::new(),
            bytes: 0,
            started: false,
            terminal: false,
            reason: None,
            blocks: BTreeMap::new(),
            usage: InferenceUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_hit_tokens: None,
            },
            refused: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderFragment>> {
        self.bytes = self.bytes.saturating_add(chunk.len());
        if self.bytes > MAX_REQUEST_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "provider response exceeds budget",
            ));
        }
        self.buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(index) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line = self.buffer.drain(..=index).collect::<Vec<_>>();
            output.extend(self.line(&line[..line.len() - 1])?);
        }
        if self.buffer.len().saturating_add(self.data.len()) > MAX_MESSAGE_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "SSE event exceeds budget",
            ));
        }
        Ok(output)
    }

    pub fn finish(mut self) -> Result<Vec<ProviderFragment>> {
        let mut output = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            output.extend(self.line(&line)?);
        }
        output.extend(self.dispatch()?);
        if !self.terminal {
            return Err(incomplete("provider stream ended without terminal event"));
        }
        Ok(output)
    }

    fn line(&mut self, line: &[u8]) -> Result<Vec<ProviderFragment>> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(b":") {
            return Ok(vec![]);
        }
        if let Some(data) = line.strip_prefix(b"data:") {
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data
                .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
        } else if let Some(event) = line.strip_prefix(b"event:") {
            self.event = std::str::from_utf8(event.strip_prefix(b" ").unwrap_or(event))
                .map_err(|_| decode("invalid SSE event name"))?
                .to_owned();
        }
        if self.data.len().saturating_add(self.event.len()) > MAX_MESSAGE_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "SSE event exceeds budget",
            ));
        }
        Ok(vec![])
    }

    fn dispatch(&mut self) -> Result<Vec<ProviderFragment>> {
        let data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if data.is_empty() {
            return Ok(vec![]);
        }
        if self.terminal {
            return Err(decode("data after provider terminal"));
        }
        if self.format == ApiFormat::ChatCompletions && data == b"[DONE]" {
            if !self.started {
                return Err(incomplete("Chat DONE before a choice"));
            }
            let reason = self
                .reason
                .clone()
                .ok_or_else(|| incomplete("Chat DONE lacks finish_reason"))?;
            self.terminal = true;
            return Ok(vec![ProviderFragment::Finished(reason)]);
        }
        let value: Value =
            serde_json::from_slice(&data).map_err(|_| decode("invalid provider SSE JSON"))?;
        if self.format == ApiFormat::ChatCompletions {
            return self.chat_event(&value);
        }
        let kind = event_string(&value, "type")?;
        if !event.is_empty() && event != kind {
            return Err(decode("SSE event and payload type disagree"));
        }
        if kind == "error" || kind == "response.failed" {
            return Err(decode("provider reported stream failure"));
        }
        match self.format {
            ApiFormat::Messages => self.messages_event(kind, &value),
            ApiFormat::Responses => self.responses_event(kind, &value),
            _ => unreachable!(),
        }
    }

    fn chat_event(&mut self, value: &Value) -> Result<Vec<ProviderFragment>> {
        if value.get("error").is_some() {
            return Err(decode("provider reported Chat error"));
        }
        let mut output = Vec::new();
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            let input_tokens = usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .and_then(Value::as_u64)
                .ok_or_else(|| decode("invalid input token usage"))?;
            let output_tokens = usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
                .and_then(Value::as_u64)
                .ok_or_else(|| decode("invalid output token usage"))?;
            let cached = usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
                .or_else(|| usage.get("prompt_cache_hit_tokens"));
            let cache_hit_tokens = cached
                .map(|value| value.as_u64().ok_or_else(|| decode("invalid cache usage")))
                .transpose()?;
            output.push(ProviderFragment::Usage(InferenceUsage {
                input_tokens,
                output_tokens,
                cache_hit_tokens,
            }));
        }
        let choices = value["choices"]
            .as_array()
            .ok_or_else(|| decode("Chat choices missing"))?;
        if choices.len() > 1 {
            return Err(decode("multiple Chat choices unsupported"));
        }
        for choice in choices {
            if choice
                .get("index")
                .is_some_and(|index| index.as_u64() != Some(0))
            {
                return Err(decode("unexpected Chat choice index"));
            }
            self.started = true;
            if let Some(previous) = &self.reason {
                let delta = choice.get("delta").or_else(|| choice.get("message"));
                let empty_delta = delta.is_some_and(|delta| {
                    delta.as_object().is_some_and(|fields| {
                        fields.iter().all(|(key, value)| match key.as_str() {
                            "content" | "reasoning" | "reasoning_content" => {
                                value.is_null() || value.as_str() == Some("")
                            }
                            "role" => value.is_null() || value.as_str() == Some("assistant"),
                            "tool_calls" => {
                                value.is_null() || value.as_array().is_some_and(Vec::is_empty)
                            }
                            _ => value.is_null(),
                        })
                    })
                });
                let repeated_reason = choice
                    .get("finish_reason")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value
                            .as_str()
                            .map(stop_reason)
                            .ok_or_else(|| decode("invalid finish_reason"))
                    })
                    .transpose()?;
                if !empty_delta || repeated_reason.is_some_and(|reason| reason != *previous) {
                    return Err(decode("Chat choice after finish_reason"));
                }
                continue;
            }
            let delta = choice
                .get("delta")
                .or_else(|| choice.get("message"))
                .ok_or_else(|| decode("Chat delta missing"))?;
            if let Some(text) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .filter(|value| !value.is_null())
            {
                output.push(ProviderFragment::Reasoning(
                    text.as_str()
                        .ok_or_else(|| decode("invalid Chat reasoning"))?
                        .to_owned(),
                ));
            }
            if let Some(text) = delta.get("content").filter(|value| !value.is_null()) {
                output.push(ProviderFragment::Content(
                    text.as_str()
                        .ok_or_else(|| decode("invalid Chat text"))?
                        .to_owned(),
                ));
            }
            if let Some(calls) = delta.get("tool_calls").filter(|value| !value.is_null()) {
                for call in calls
                    .as_array()
                    .ok_or_else(|| decode("invalid tool call deltas"))?
                {
                    let index = event_index(call, "index")?;
                    let id = call
                        .get("id")
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| decode("invalid tool call id"))
                        })
                        .transpose()?;
                    let function = call.get("function");
                    let name = function
                        .and_then(|value| value.get("name"))
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| decode("invalid tool name"))
                        })
                        .transpose()?;
                    let arguments = function
                        .and_then(|value| value.get("arguments"))
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| decode("invalid tool arguments delta"))
                        })
                        .transpose()?
                        .unwrap_or_default();
                    output.push(ProviderFragment::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments,
                    });
                }
            }
            if let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null()) {
                self.reason = Some(stop_reason(
                    reason
                        .as_str()
                        .ok_or_else(|| decode("invalid finish_reason"))?,
                ));
            }
        }
        Ok(output)
    }

    fn messages_usage(&mut self, value: &Value) -> Result<ProviderFragment> {
        if !value.is_object() {
            return Err(decode("Messages usage missing"));
        }
        let mut input_changed = false;
        if let Some(input) = value.get("input_tokens") {
            self.usage.input_tokens = input
                .as_u64()
                .ok_or_else(|| decode("invalid input usage"))?;
            input_changed = true;
        }
        if let Some(output) = value.get("output_tokens") {
            self.usage.output_tokens = output
                .as_u64()
                .ok_or_else(|| decode("invalid output usage"))?;
        }
        if let Some(cached) = value.get("cache_read_input_tokens") {
            self.usage.cache_hit_tokens = Some(
                cached
                    .as_u64()
                    .ok_or_else(|| decode("invalid cache usage"))?,
            );
        }
        if input_changed {
            let created = value
                .get("cache_creation_input_tokens")
                .map(|value| {
                    value
                        .as_u64()
                        .ok_or_else(|| decode("invalid cache creation usage"))
                })
                .transpose()?
                .unwrap_or(0);
            self.usage.input_tokens = self
                .usage
                .input_tokens
                .checked_add(self.usage.cache_hit_tokens.unwrap_or(0))
                .and_then(|value| value.checked_add(created))
                .ok_or_else(|| decode("token usage overflow"))?;
        }
        Ok(ProviderFragment::Usage(self.usage.clone()))
    }

    fn messages_event(&mut self, kind: &str, value: &Value) -> Result<Vec<ProviderFragment>> {
        if kind == "ping" {
            return Ok(vec![]);
        }
        if kind == "message_start" {
            if self.started
                || value["message"]["role"] != "assistant"
                || !value["message"]["content"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
            {
                return Err(decode("invalid Messages start"));
            }
            self.started = true;
            return Ok(vec![self.messages_usage(&value["message"]["usage"])?]);
        }
        if !self.started {
            return Err(decode("Messages event before start"));
        }
        match kind {
            "content_block_start" => {
                if self.reason.is_some() {
                    return Err(decode("content after message_delta"));
                }
                let index = event_index(value, "index")?;
                let block = &value["content_block"];
                let kind = event_string(block, "type")?.to_owned();
                let mut state = BlockState {
                    kind: kind.clone(),
                    ..Default::default()
                };
                let mut output = Vec::new();
                match kind.as_str() {
                    "text" => output.push(ProviderFragment::Content(event_string(block, "text")?.to_owned())),
                    "thinking" => {
                        let thinking = event_string(block, "thinking")?;
                        output.push(ProviderFragment::Reasoning(thinking.to_owned()));
                        output.push(ProviderFragment::Replay(json!({"api_format":"messages","index":index,"op":"start","item":{"type":"thinking","thinking":thinking,"signature":block.get("signature").and_then(Value::as_str).unwrap_or("")}})));
                    }
                    "redacted_thinking" => output.push(ProviderFragment::Replay(json!({"api_format":"messages","index":index,"op":"start","item":{"type":"redacted_thinking","data":event_string(block,"data")?}}))),
                    "tool_use" => {
                        state.id = event_string(block, "id")?.to_owned();
                        state.name = event_string(block, "name")?.to_owned();
                        if !block["input"].is_object() { return Err(decode("invalid Messages tool input")); }
                        let arguments = if block["input"] == json!({}) { String::new() } else { state.has_arguments = true; block["input"].to_string() };
                        output.push(ProviderFragment::ToolCallDelta { index, id: Some(state.id.clone()), name: Some(state.name.clone()), arguments });
                    }
                    _ => return Err(decode("unsupported Messages block")),
                }
                if self.blocks.insert(index, state).is_some() {
                    return Err(decode("duplicate Messages block"));
                }
                Ok(output)
            }
            "content_block_delta" => {
                let index = event_index(value, "index")?;
                let state = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| decode("delta for unknown Messages block"))?;
                if state.closed {
                    return Err(decode("delta for closed Messages block"));
                }
                let delta = &value["delta"];
                match (event_string(delta, "type")?, state.kind.as_str()) {
                    ("text_delta", "text") => Ok(vec![ProviderFragment::Content(
                        event_string(delta, "text")?.to_owned(),
                    )]),
                    ("thinking_delta", "thinking") => {
                        let text = event_string(delta, "thinking")?;
                        Ok(vec![
                            ProviderFragment::Reasoning(text.to_owned()),
                            ProviderFragment::Replay(
                                json!({"api_format":"messages","index":index,"op":"thinking_delta","value":text}),
                            ),
                        ])
                    }
                    ("signature_delta", "thinking") => Ok(vec![ProviderFragment::Replay(
                        json!({"api_format":"messages","index":index,"op":"signature_delta","value":event_string(delta,"signature")?}),
                    )]),
                    ("input_json_delta", "tool_use") => {
                        state.has_arguments = true;
                        Ok(vec![ProviderFragment::ToolCallDelta {
                            index,
                            id: None,
                            name: None,
                            arguments: event_string(delta, "partial_json")?.to_owned(),
                        }])
                    }
                    _ => Err(decode("Messages delta does not match block type")),
                }
            }
            "content_block_stop" => {
                let index = event_index(value, "index")?;
                let state = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| decode("stop for unknown Messages block"))?;
                if state.closed {
                    return Err(decode("duplicate Messages block stop"));
                }
                state.closed = true;
                if state.kind == "tool_use" && !state.has_arguments {
                    return Ok(vec![ProviderFragment::ToolCallDelta {
                        index,
                        id: None,
                        name: None,
                        arguments: "{}".into(),
                    }]);
                }
                Ok(vec![])
            }
            "message_delta" => {
                if self.reason.is_some() || self.blocks.values().any(|block| !block.closed) {
                    return Err(decode("message_delta before block completion or duplicate"));
                }
                self.reason = Some(stop_reason(event_string(&value["delta"], "stop_reason")?));
                Ok(vec![self.messages_usage(&value["usage"])?])
            }
            "message_stop" => {
                if self.blocks.values().any(|block| !block.closed) {
                    return Err(incomplete("Messages terminal with open blocks"));
                }
                let reason = self
                    .reason
                    .clone()
                    .ok_or_else(|| incomplete("Messages terminal without stop_reason"))?;
                self.terminal = true;
                Ok(vec![ProviderFragment::Finished(reason)])
            }
            _ => Err(decode("unsupported Messages event")),
        }
    }

    fn responses_event(&mut self, kind: &str, value: &Value) -> Result<Vec<ProviderFragment>> {
        if kind == "response.created" {
            if self.started {
                return Err(decode("duplicate response.created"));
            }
            self.started = true;
            return Ok(vec![]);
        }
        if !self.started {
            return Err(decode("Responses event before response.created"));
        }
        match kind {
            "response.in_progress" => Ok(vec![]),
            "response.output_item.added" => {
                let index = event_index(value, "output_index")?;
                let item = &value["item"];
                let kind = event_string(item, "type")?;
                if !matches!(kind, "message" | "reasoning" | "function_call") {
                    return Err(decode("unsupported Responses output item"));
                }
                if item
                    .get("content")
                    .is_some_and(|parts| !parts.as_array().is_some_and(Vec::is_empty))
                    || item
                        .get("summary")
                        .is_some_and(|parts| !parts.as_array().is_some_and(Vec::is_empty))
                {
                    return Err(decode("initial output item bypasses streamed deltas"));
                }
                let mut state = BlockState {
                    kind: kind.to_owned(),
                    ..Default::default()
                };
                let mut output = Vec::new();
                if kind == "reasoning" {
                    state.id = event_string(item, "id")?.to_owned();
                    state.encrypted = item.get("encrypted_content").cloned();
                }
                if kind == "function_call" {
                    state.id = event_string(item, "call_id")?.to_owned();
                    state.name = event_string(item, "name")?.to_owned();
                    let arguments = event_string(item, "arguments")?.to_owned();
                    state.arguments.update(arguments.as_bytes());
                    output.push(ProviderFragment::ToolCallDelta {
                        index,
                        id: Some(state.id.clone()),
                        name: Some(state.name.clone()),
                        arguments,
                    });
                }
                if self.blocks.insert(index, state).is_some() {
                    return Err(decode("duplicate Responses output index"));
                }
                Ok(output)
            }
            "response.output_text.delta"
            | "response.refusal.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta" => {
                let index = event_index(value, "output_index")?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| decode("delta for unknown Responses item"))?;
                let reasoning = kind.contains("reasoning");
                if block.closed
                    || (reasoning && block.kind != "reasoning")
                    || (!reasoning && block.kind != "message")
                {
                    return Err(decode("Responses text delta does not match item"));
                }
                let text = event_string(value, "delta")?;
                if kind == "response.reasoning_text.delta" {
                    block.reasoning.update(text.as_bytes());
                } else {
                    block.text.update(text.as_bytes());
                }
                if kind == "response.refusal.delta" {
                    self.refused = true;
                }
                Ok(vec![if reasoning {
                    ProviderFragment::Reasoning(text.to_owned())
                } else {
                    ProviderFragment::Content(text.to_owned())
                }])
            }
            "response.function_call_arguments.delta" => {
                let index = event_index(value, "output_index")?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| decode("arguments for unknown Responses item"))?;
                if block.closed || block.kind != "function_call" {
                    return Err(decode("Responses arguments do not match item"));
                }
                let arguments = event_string(value, "delta")?.to_owned();
                block.arguments.update(arguments.as_bytes());
                Ok(vec![ProviderFragment::ToolCallDelta {
                    index,
                    id: None,
                    name: None,
                    arguments,
                }])
            }
            "response.function_call_arguments.done" => {
                let block = self
                    .blocks
                    .get(&event_index(value, "output_index")?)
                    .ok_or_else(|| decode("arguments done for unknown item"))?;
                if block.closed
                    || block.kind != "function_call"
                    || !hash_matches(&block.arguments, event_string(value, "arguments")?)
                {
                    return Err(decode("Responses completed arguments disagree with deltas"));
                }
                Ok(vec![])
            }
            "response.output_item.done" => {
                let index = event_index(value, "output_index")?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| decode("completion for unknown Responses item"))?;
                let item = &value["item"];
                if block.closed || item["type"] != block.kind {
                    return Err(decode("invalid Responses item completion"));
                }
                if block.kind == "function_call"
                    && (item["call_id"] != block.id
                        || item["name"] != block.name
                        || !hash_matches(&block.arguments, event_string(item, "arguments")?))
                {
                    return Err(decode("Responses completed call disagrees with deltas"));
                }
                if block.kind == "message" || block.kind == "reasoning" {
                    let channels = if block.kind == "message" {
                        vec![("content", &block.text)]
                    } else {
                        vec![("summary", &block.text), ("content", &block.reasoning)]
                    };
                    for (channel, streamed) in channels {
                        let mut digest = Sha256::new();
                        if let Some(parts) = item.get(channel) {
                            for part in parts
                                .as_array()
                                .ok_or_else(|| decode("invalid completed content parts"))?
                            {
                                let field = match (
                                    block.kind.as_str(),
                                    channel,
                                    event_string(part, "type")?,
                                ) {
                                    ("message", "content", "output_text")
                                    | ("reasoning", "summary", "summary_text")
                                    | ("reasoning", "content", "reasoning_text") => "text",
                                    ("message", "content", "refusal") => "refusal",
                                    _ => return Err(decode("unsupported completed content part")),
                                };
                                digest.update(event_string(part, field)?.as_bytes());
                            }
                        }
                        if digest.finalize() != streamed.clone().finalize() {
                            return Err(decode("completed Responses text disagrees with deltas"));
                        }
                    }
                }
                block.closed = true;
                if block.kind == "reasoning" {
                    if item["id"] != block.id {
                        return Err(decode("reasoning item identity changed"));
                    }
                    let mut replay = json!({"type":"reasoning","id":block.id,"summary":item.get("summary").cloned().unwrap_or_else(|| json!([]))});
                    if let Some(content) = item.get("content") {
                        replay["content"] = content.clone();
                    }
                    if let Some(encrypted) =
                        item.get("encrypted_content").or(block.encrypted.as_ref())
                    {
                        replay["encrypted_content"] = encrypted.clone();
                    }
                    return Ok(vec![ProviderFragment::Replay(
                        json!({"api_format":"responses","index":index,"op":"replace","item":replay}),
                    )]);
                }
                Ok(vec![])
            }
            "response.completed" | "response.incomplete" => {
                if self.blocks.values().any(|block| !block.closed) {
                    return Err(incomplete("Responses terminal has open output items"));
                }
                let response = &value["response"];
                let expected = if kind == "response.completed" {
                    "completed"
                } else {
                    "incomplete"
                };
                if response["status"] != expected {
                    return Err(decode("Responses terminal status mismatch"));
                }
                let reason = if kind == "response.incomplete" {
                    stop_reason(event_string(&response["incomplete_details"], "reason")?)
                } else if self.refused {
                    InferenceStopReason::ContentFilter
                } else if self
                    .blocks
                    .values()
                    .any(|block| block.kind == "function_call")
                {
                    InferenceStopReason::ToolCalls
                } else {
                    InferenceStopReason::Completed
                };
                let mut output = Vec::new();
                if let Some(usage) = response.get("usage").filter(|usage| !usage.is_null()) {
                    let input_tokens = usage["input_tokens"]
                        .as_u64()
                        .ok_or_else(|| decode("invalid Responses input usage"))?;
                    let output_tokens = usage["output_tokens"]
                        .as_u64()
                        .ok_or_else(|| decode("invalid Responses output usage"))?;
                    let cache_hit_tokens = usage
                        .pointer("/input_tokens_details/cached_tokens")
                        .map(|value| {
                            value
                                .as_u64()
                                .ok_or_else(|| decode("invalid Responses cached token usage"))
                        })
                        .transpose()?;
                    output.push(ProviderFragment::Usage(InferenceUsage {
                        input_tokens,
                        output_tokens,
                        cache_hit_tokens,
                    }));
                }
                self.terminal = true;
                output.push(ProviderFragment::Finished(reason));
                Ok(output)
            }
            "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.done"
            | "response.refusal.done" => {
                let block = self
                    .blocks
                    .get(&event_index(value, "output_index")?)
                    .ok_or_else(|| decode("event for unknown Responses item"))?;
                if block.closed {
                    return Err(decode("event for closed Responses item"));
                }
                Ok(vec![])
            }
            _ => Err(decode("unsupported Responses event")),
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/adapters.rs"]
mod adapter_tests;

#[derive(Clone)]
pub(crate) struct InferenceLaunchOutput;
#[derive(Clone)]
pub(crate) struct InferenceLaunchError {
    pub fallback: Option<crate::handler::InferenceLaunchFallback>,
    pub error: InferenceError,
}
impl From<async_runtime_plugin::AsyncTaskError> for InferenceLaunchError {
    fn from(error: async_runtime_plugin::AsyncTaskError) -> Self {
        Self {
            fallback: None,
            error: InferenceError::new(InferenceErrorKind::TaskPanicked, error.to_string()),
        }
    }
}

/// Constructed only when the queued launch is dispatched, never while queued.
pub(crate) enum InferenceCleanupSender {
    Async(async_runtime_plugin::AsyncContext),
    #[cfg(test)]
    Runtime(RuntimeEventSender),
}
impl InferenceCleanupSender {
    pub fn send_event(&self, event: InferenceCleanupFinished) {
        match self {
            Self::Async(context) => context.send_event(event),
            #[cfg(test)]
            Self::Runtime(events) => events.send_event(event),
        }
    }
}
