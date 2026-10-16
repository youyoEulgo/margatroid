use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{InferenceError, InferenceErrorKind};
use margatroid_types::message_types::{
    validate_user_images, AgentMessage, TokenUsage, ToolCall, UserImage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    ProviderInferenceResponse, ProviderResponseAccumulator, ProviderStreamDelta, StopReason,
};
use margatroid_types::inference_request_types::DomainRequest;

type Result<T> = std::result::Result<T, InferenceError>;

fn invalid(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::InvalidParameters, message)
}
fn decode(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::ResponseDecodeFailed, message)
}
fn incomplete(message: &'static str) -> InferenceError {
    InferenceError::new(InferenceErrorKind::ResponseIncomplete, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    ChatCompletions,
    Messages,
    Responses,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum ApiRequest {
    ChatCompletions(ChatCompletionsRequest),
    Messages(MessagesRequest),
    Responses(ResponsesRequest),
}

impl ApiFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::Responses => "responses",
        }
    }

    pub fn build_request(self, request: DomainRequest) -> Result<ApiRequest> {
        match self {
            Self::ChatCompletions => Ok(ApiRequest::ChatCompletions(request.try_into()?)),
            Self::Messages => Ok(ApiRequest::Messages(request.try_into()?)),
            Self::Responses => Ok(ApiRequest::Responses(request.try_into()?)),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
    pub stream_options: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<Value>,
    pub messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
    pub store: bool,
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

fn normalize(request: &mut DomainRequest) -> Result<()> {
    super::ModelId::new(request.config.model.clone())?;
    super::InferenceParameters::new(
        request.config.temperature,
        request.config.max_output_tokens,
        request.config.top_p,
        request.config.stop.clone(),
    )
    .validate()?;
    fn flatten(
        messages: Vec<AgentMessage>,
        output: &mut Vec<AgentMessage>,
        depth: usize,
    ) -> Result<()> {
        if depth > 32 {
            return Err(invalid("message nesting exceeds limit"));
        }
        for message in messages {
            if matches!(
                &message,
                AgentMessage::Assistant {
                    stop_reason: Some(StopReason::Cancelled),
                    ..
                }
            ) {
                return Err(invalid(
                    "cancelled assistant cannot enter inference context",
                ));
            }
            match message {
                AgentMessage::Inject { messages } => flatten(messages, output, depth + 1)?,
                AgentMessage::Error { message } => output.push(AgentMessage::User {
                    content: message,
                    images: vec![],
                }),
                other => output.push(other),
            }
        }
        Ok(())
    }
    let mut messages = Vec::new();
    flatten(std::mem::take(&mut request.messages), &mut messages, 0)?;
    if messages.is_empty() {
        return Err(invalid("request has no messages"));
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
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !names.insert(&tool.name)
            || !tool.input_schema.is_object()
        {
            return Err(invalid("invalid or duplicate tool definition"));
        }
    }
    let mut pending = HashMap::new();
    let mut ids = HashSet::new();
    let mut bytes = 0usize;
    let mut encoded_bytes = 0usize;
    for message in &messages {
        let (size, encoded_size) = validated_message_sizes(message)?;
        bytes = bytes.saturating_add(size);
        encoded_bytes = encoded_bytes.saturating_add(encoded_size);
        if size > super::MAX_MESSAGE_BYTES
            || bytes > super::MAX_MESSAGES_BYTES
            || encoded_bytes > super::MAX_REQUEST_BYTES
            || messages.len() > 4096
        {
            return Err(invalid("messages exceed limit"));
        }
        match message {
            AgentMessage::Assistant { tool_calls, .. } => {
                if !pending.is_empty() {
                    return Err(invalid("tool batch is missing results"));
                }
                if tool_calls.len() > 256 {
                    return Err(invalid("too many tool calls"));
                }
                for call in tool_calls {
                    if call.id.is_empty()
                        || call.id.len() > 256
                        || call.tool_name.is_empty()
                        || !ids.insert(call.id.clone())
                        || !serde_json::from_str::<Value>(&call.arguments)
                            .is_ok_and(|v| v.is_object())
                    {
                        return Err(invalid("invalid tool call"));
                    }
                    pending.insert(call.id.clone(), call.tool_name.clone());
                }
            }
            AgentMessage::ToolCall {
                tool_call_id,
                tool_name,
                ..
            } => {
                if pending.remove(tool_call_id).as_deref() != Some(tool_name.as_str()) {
                    return Err(invalid("tool result does not match a pending tool call"));
                }
            }
            _ if !pending.is_empty() => {
                return Err(invalid("tool batch interrupted before all results"))
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err(invalid("tool batch is missing results"));
    }
    request.messages = messages;
    Ok(())
}

pub(crate) fn validated_message_sizes(message: &AgentMessage) -> Result<(usize, usize)> {
    if let AgentMessage::User { images, .. } | AgentMessage::ToolCall { images, .. } = message {
        validate_user_images(images)
            .map_err(|error| InferenceError::new(InferenceErrorKind::InvalidMessages, error))?;
    }
    let encoded = serde_json::to_vec(message)
        .map_err(|_| invalid("invalid message"))?
        .len();
    let text = match message {
        AgentMessage::User { content, images } if !images.is_empty() => {
            serde_json::to_vec(&AgentMessage::User {
                content: content.clone(),
                images: vec![],
            })
            .map_err(|_| invalid("invalid message"))?
            .len()
        }
        AgentMessage::ToolCall { images, .. } if !images.is_empty() => {
            let mut text_only = message.clone();
            if let AgentMessage::ToolCall { images, .. } = &mut text_only {
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

fn user_content(content: String, images: Vec<UserImage>, format: ApiFormat) -> Value {
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

fn replay_items(
    replay: Option<Value>,
    format: ApiFormat,
    reasoning: Option<&str>,
) -> Result<Vec<Value>> {
    let Some(replay) = replay else {
        if reasoning.is_some_and(|text| !text.is_empty()) {
            return Err(invalid("historical reasoning requires API replay metadata"));
        }
        return Ok(Vec::new());
    };
    if replay["api_format"] != format.as_str() {
        return Err(invalid("historical reasoning replay API format mismatch"));
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

impl TryFrom<DomainRequest> for ChatCompletionsRequest {
    type Error = InferenceError;
    fn try_from(mut request: DomainRequest) -> Result<Self> {
        normalize(&mut request)?;
        if request.messages.iter().any(|message| {
            matches!(
                message,
                AgentMessage::Assistant {
                    replay: Some(_),
                    ..
                }
            )
        }) {
            return Err(invalid(
                "Chat Completions cannot replay foreign reasoning metadata",
            ));
        }
        let mut messages = Vec::new();
        let mut pending = HashSet::new();
        let mut image_blocks = Vec::new();
        for message in request.messages {
            let value = match message {
                AgentMessage::System { content } => json!({"role":"system","content":content}),
                AgentMessage::User { content, images } => {
                    json!({"role":"user","content":user_content(content, images, ApiFormat::ChatCompletions)})
                }
                AgentMessage::Assistant {
                    reasoning,
                    content,
                    tool_calls,
                    ..
                } => {
                    pending.extend(tool_calls.iter().map(|call| call.id.clone()));
                    let mut value = json!({"role":"assistant","content":content});
                    if let Some(reasoning) = reasoning {
                        value["reasoning_content"] = json!(reasoning);
                    }
                    if !tool_calls.is_empty() {
                        value["tool_calls"] = json!(tool_calls.into_iter().map(|call| json!({"id":call.id,"type":"function","function":{"name":call.tool_name,"arguments":call.arguments}})).collect::<Vec<_>>());
                    }
                    value
                }
                AgentMessage::ToolCall {
                    tool_call_id,
                    content,
                    images,
                    ..
                } => {
                    pending.remove(&tool_call_id);
                    for image in images {
                        image_blocks.push(
                            json!({"type":"text","text":format!("tool_call_id: {tool_call_id}")}),
                        );
                        image_blocks.extend(
                            user_content(String::new(), vec![image], ApiFormat::ChatCompletions)
                                .as_array()
                                .unwrap()
                                .iter()
                                .cloned(),
                        );
                    }
                    json!({"role":"tool","tool_call_id":tool_call_id,"content":content})
                }
                _ => unreachable!(),
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
                AgentMessage::System { content } => {
                    if !messages.is_empty() {
                        return Err(invalid(
                            "Messages cannot preserve interleaved system messages",
                        ));
                    }
                    system.push(json!({"type":"text","text":content}));
                }
                AgentMessage::User { content, images } => merge_message(
                    &mut messages,
                    "user",
                    user_content(content, images, ApiFormat::Messages)
                        .as_array()
                        .unwrap()
                        .clone(),
                ),
                AgentMessage::Assistant {
                    reasoning,
                    replay,
                    content,
                    tool_calls,
                    ..
                } => {
                    let mut blocks =
                        replay_items(replay, ApiFormat::Messages, reasoning.as_deref())?;
                    if let Some(content) = content {
                        blocks.push(json!({"type":"text","text":content}));
                    }
                    for call in tool_calls {
                        blocks.push(json!({"type":"tool_use","id":call.id,"name":call.tool_name,"input":serde_json::from_str::<Value>(&call.arguments).map_err(|_| invalid("invalid tool arguments"))?}));
                    }
                    if blocks.is_empty() {
                        return Err(invalid("Messages assistant has no encodable content"));
                    }
                    merge_message(&mut messages, "assistant", blocks);
                }
                AgentMessage::ToolCall {
                    tool_call_id,
                    content,
                    images,
                    failed,
                    ..
                } => merge_message(
                    &mut messages,
                    "user",
                    vec![
                        json!({"type":"tool_result","tool_use_id":tool_call_id,"content":if images.is_empty() { json!(content) } else { user_content(content, images, ApiFormat::Messages) },"is_error":failed}),
                    ],
                ),
                _ => unreachable!(),
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
                AgentMessage::System { content } => {
                    input.push(json!({"role":"system","content":content}))
                }
                AgentMessage::User { content, images } => {
                    input.push(json!({"role":"user","content":user_content(content, images, ApiFormat::Responses)}))
                }
                AgentMessage::Assistant {
                    reasoning,
                    replay,
                    content,
                    tool_calls,
                    ..
                } => {
                    input.extend(replay_items(
                        replay,
                        ApiFormat::Responses,
                        reasoning.as_deref(),
                    )?);
                    if let Some(content) = content {
                        input.push(json!({"role":"assistant","content":[{"type":"output_text","text":content}]}));
                    }
                    for call in tool_calls {
                        input.push(json!({"type":"function_call","call_id":call.id,"name":call.tool_name,"arguments":call.arguments}));
                    }
                }
                AgentMessage::ToolCall {
                    tool_call_id,
                    content,
                    images,
                    ..
                } => input.push(
                    json!({"type":"function_call_output","call_id":tool_call_id,"output":user_content(content, images, ApiFormat::Responses)}),
                ),
                _ => unreachable!(),
            }
        }
        Ok(Self { model:request.config.model,input,tools:request.tools.into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.input_schema,"strict":false})).collect(),stream:true,store:false,include:vec!["reasoning.encrypted_content".into()],temperature:request.config.temperature,max_output_tokens:request.config.max_output_tokens,top_p:request.config.top_p })
    }
}

pub(crate) fn new_messages_accumulator() -> Box<dyn ProviderResponseAccumulator> {
    Box::new(FormatAccumulator::new(ApiFormat::Messages))
}
pub(crate) fn new_responses_accumulator() -> Box<dyn ProviderResponseAccumulator> {
    Box::new(FormatAccumulator::new(ApiFormat::Responses))
}

#[derive(Default)]
struct Block {
    kind: String,
    id: String,
    name: String,
    arguments: String,
    text: String,
    raw_reasoning: String,
    replay: Option<Value>,
    closed: bool,
}

struct FormatAccumulator {
    format: ApiFormat,
    buffer: Vec<u8>,
    data: Vec<u8>,
    event: String,
    total: usize,
    started: bool,
    terminal: bool,
    refused: bool,
    reason: Option<StopReason>,
    content: String,
    reasoning: String,
    blocks: BTreeMap<usize, Block>,
    usage: Option<TokenUsage>,
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| decode("missing or invalid event string field"))
}
fn index(value: &Value, key: &str) -> Result<usize> {
    let n = value[key]
        .as_u64()
        .ok_or_else(|| decode("missing event index"))?;
    if n >= 256 {
        return Err(decode("event index exceeds limit"));
    }
    Ok(n as usize)
}

impl FormatAccumulator {
    fn new(format: ApiFormat) -> Self {
        Self {
            format,
            buffer: Vec::new(),
            data: Vec::new(),
            event: String::new(),
            total: 0,
            started: false,
            terminal: false,
            refused: false,
            reason: None,
            content: String::new(),
            reasoning: String::new(),
            blocks: BTreeMap::new(),
            usage: None,
        }
    }
    fn line(&mut self, line: &[u8]) -> Result<Vec<ProviderStreamDelta>> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return self.dispatch();
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
        Ok(Vec::new())
    }
    fn dispatch(&mut self) -> Result<Vec<ProviderStreamDelta>> {
        let data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if data.is_empty() {
            return Ok(Vec::new());
        }
        if self.terminal {
            return Err(decode("event after terminal event"));
        }
        let value: Value = serde_json::from_slice(&data).map_err(|_| decode("invalid SSE JSON"))?;
        let kind = string(&value, "type")?;
        if !event.is_empty() && event != kind {
            return Err(decode("SSE event and payload type disagree"));
        }
        if kind == "error" || kind == "response.failed" {
            return Err(decode("provider reported a stream error"));
        }
        match self.format {
            ApiFormat::Messages => self.messages_event(kind, &value),
            ApiFormat::Responses => self.responses_event(kind, &value),
            _ => unreachable!(),
        }
    }
    fn text(&mut self, text: &str, reasoning: bool) -> Vec<ProviderStreamDelta> {
        if reasoning {
            self.reasoning.push_str(text);
            vec![ProviderStreamDelta::Reasoning(text.to_owned())]
        } else {
            self.content.push_str(text);
            vec![ProviderStreamDelta::Content(text.to_owned())]
        }
    }
    fn messages_event(&mut self, kind: &str, value: &Value) -> Result<Vec<ProviderStreamDelta>> {
        if kind == "ping" {
            return Ok(Vec::new());
        }
        if kind == "message_start" {
            if self.started {
                return Err(decode("duplicate message_start"));
            }
            if value["message"]["role"] != "assistant"
                || !value["message"]["content"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
            {
                return Err(decode("invalid message_start"));
            }
            self.started = true;
            self.messages_usage(&value["message"]["usage"])?;
            return Ok(Vec::new());
        }
        if !self.started {
            return Err(decode("message event before message_start"));
        }
        match kind {
            "content_block_start" => {
                if self.reason.is_some() {
                    return Err(decode("content after message_delta"));
                }
                let i = index(value, "index")?;
                let block = &value["content_block"];
                let kind = string(block, "type")?.to_owned();
                if !matches!(
                    kind.as_str(),
                    "text" | "thinking" | "redacted_thinking" | "tool_use"
                ) {
                    return Err(decode("unsupported Messages content block"));
                }
                let mut builder = Block {
                    kind: kind.clone(),
                    ..Block::default()
                };
                let mut output = Vec::new();
                match kind.as_str() {
                    "tool_use" => {
                        builder.id = string(block, "id")?.into();
                        builder.name = string(block, "name")?.into();
                        if !block["input"].is_object() {
                            return Err(decode("invalid tool input"));
                        }
                        if block["input"] != json!({}) {
                            builder.arguments = block["input"].to_string();
                        }
                    }
                    "text" => output = self.text(string(block, "text")?, false),
                    "thinking" => {
                        let thinking = string(block, "thinking")?;
                        builder.replay = Some(
                            json!({"type":"thinking","thinking":thinking,"signature":block.get("signature").and_then(Value::as_str).unwrap_or("")}),
                        );
                        output = self.text(thinking, true);
                    }
                    "redacted_thinking" => {
                        builder.replay =
                            Some(json!({"type":"redacted_thinking","data":string(block,"data")?}));
                    }
                    _ => unreachable!(),
                }
                if self.blocks.insert(i, builder).is_some() {
                    return Err(decode("duplicate content block index"));
                }
                Ok(output)
            }
            "content_block_delta" => {
                let i = index(value, "index")?;
                let block = self
                    .blocks
                    .get_mut(&i)
                    .ok_or_else(|| decode("delta for unknown content block"))?;
                if block.closed {
                    return Err(decode("delta for closed content block"));
                }
                let delta = &value["delta"];
                match string(delta, "type")? {
                    "text_delta" if block.kind == "text" => {
                        Ok(self.text(string(delta, "text")?, false))
                    }
                    "thinking_delta" if block.kind == "thinking" => {
                        let text = string(delta, "thinking")?;
                        let replay = block.replay.as_mut().unwrap();
                        let mut thinking = replay["thinking"].as_str().unwrap().to_owned();
                        thinking.push_str(text);
                        replay["thinking"] = json!(thinking);
                        Ok(self.text(text, true))
                    }
                    "signature_delta" if block.kind == "thinking" => {
                        let replay = block.replay.as_mut().unwrap();
                        let mut signature = replay["signature"].as_str().unwrap().to_owned();
                        signature.push_str(string(delta, "signature")?);
                        replay["signature"] = json!(signature);
                        Ok(Vec::new())
                    }
                    "input_json_delta" if block.kind == "tool_use" => {
                        block.arguments.push_str(string(delta, "partial_json")?);
                        Ok(Vec::new())
                    }
                    _ => Err(decode("content delta does not match block type")),
                }
            }
            "content_block_stop" => {
                let block = self
                    .blocks
                    .get_mut(&index(value, "index")?)
                    .ok_or_else(|| decode("stop for unknown block"))?;
                if block.closed {
                    return Err(decode("duplicate block stop"));
                }
                block.closed = true;
                if block.kind == "tool_use" && block.arguments.is_empty() {
                    block.arguments = "{}".into();
                }
                Ok(Vec::new())
            }
            "message_delta" => {
                if self.blocks.values().any(|b| !b.closed) || self.reason.is_some() {
                    return Err(decode("message_delta before block completion or duplicate"));
                }
                self.reason = Some(match string(&value["delta"], "stop_reason")? {
                    "end_turn" | "stop_sequence" => StopReason::Completed,
                    "tool_use" => StopReason::ToolCalls,
                    "max_tokens" | "model_context_window_exceeded" => StopReason::Length,
                    "refusal" => StopReason::ContentFilter,
                    other => StopReason::Other(other.into()),
                });
                self.messages_usage(&value["usage"])?;
                Ok(Vec::new())
            }
            "message_stop" => {
                if self.reason.is_none() || self.blocks.values().any(|b| !b.closed) {
                    return Err(incomplete("message_stop before completion"));
                }
                self.terminal = true;
                Ok(Vec::new())
            }
            _ => Err(decode("unsupported Messages event")),
        }
    }
    fn messages_usage(&mut self, value: &Value) -> Result<()> {
        if !value.is_object() {
            return Err(decode("missing Messages usage"));
        }
        let usage = self.usage.get_or_insert_with(TokenUsage::default);
        for (key, target) in [
            ("input_tokens", &mut usage.input_tokens),
            ("output_tokens", &mut usage.output_tokens),
            ("cache_read_input_tokens", &mut usage.cache_hit_tokens),
        ] {
            if let Some(v) = value.get(key) {
                *target = v.as_u64().ok_or_else(|| decode("invalid usage count"))?;
            }
        }
        if value.get("input_tokens").is_some() {
            let creation = match value.get("cache_creation_input_tokens") {
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| decode("invalid cache creation usage"))?,
                None => 0,
            };
            usage.input_tokens = usage
                .input_tokens
                .checked_add(usage.cache_hit_tokens)
                .and_then(|n| n.checked_add(creation))
                .ok_or_else(|| decode("usage count overflow"))?;
        }
        Ok(())
    }
    fn responses_event(&mut self, kind: &str, value: &Value) -> Result<Vec<ProviderStreamDelta>> {
        if kind == "response.created" {
            if self.started {
                return Err(decode("duplicate response.created"));
            }
            self.started = true;
            return Ok(Vec::new());
        }
        if !self.started {
            return Err(decode("response event before response.created"));
        }
        match kind {
            "response.in_progress" => Ok(Vec::new()),
            "response.output_item.added" => {
                let i = index(value, "output_index")?;
                let item = &value["item"];
                let kind = string(item, "type")?;
                if !matches!(kind, "function_call" | "message" | "reasoning") {
                    return Err(decode("unsupported response output item"));
                }
                let mut block = Block {
                    kind: kind.into(),
                    ..Block::default()
                };
                if kind == "function_call" {
                    block.id = string(item, "call_id")?.into();
                    block.name = string(item, "name")?.into();
                    block.arguments = string(item, "arguments")?.into();
                }
                if kind == "reasoning" {
                    string(item, "id")?;
                    block.replay = Some(item.clone());
                }
                if item
                    .get("content")
                    .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
                    || item
                        .get("summary")
                        .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
                {
                    return Err(decode("nonempty initial response item would bypass deltas"));
                }
                if self.blocks.insert(i, block).is_some() {
                    return Err(decode("duplicate response output index"));
                }
                Ok(Vec::new())
            }
            "response.output_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.refusal.delta" => {
                let block = self
                    .blocks
                    .get(&index(value, "output_index")?)
                    .ok_or_else(|| decode("text for unknown item"))?;
                let reasoning = kind.contains("reasoning");
                if block.closed
                    || (reasoning && block.kind != "reasoning")
                    || (!reasoning && block.kind != "message")
                {
                    return Err(decode("text delta does not match item"));
                }
                let text = string(value, "delta")?;
                let block = self.blocks.get_mut(&index(value, "output_index")?).unwrap();
                if kind == "response.reasoning_text.delta" {
                    block.raw_reasoning.push_str(text);
                } else {
                    block.text.push_str(text);
                }
                if kind == "response.refusal.delta" {
                    self.refused = true;
                }
                Ok(self.text(text, reasoning))
            }
            "response.function_call_arguments.delta" => {
                let block = self
                    .blocks
                    .get_mut(&index(value, "output_index")?)
                    .ok_or_else(|| decode("arguments for unknown item"))?;
                if block.closed || block.kind != "function_call" {
                    return Err(decode("arguments do not match item"));
                }
                block.arguments.push_str(string(value, "delta")?);
                Ok(Vec::new())
            }
            "response.output_item.done" => {
                let block = self
                    .blocks
                    .get_mut(&index(value, "output_index")?)
                    .ok_or_else(|| decode("done for unknown item"))?;
                let item = &value["item"];
                if block.closed || item["type"] != block.kind {
                    return Err(decode("invalid output item completion"));
                }
                if block.kind == "function_call"
                    && (item["call_id"] != block.id
                        || item["name"] != block.name
                        || item["arguments"] != block.arguments)
                {
                    return Err(decode("completed tool call disagrees with deltas"));
                }
                if block.kind == "message" || block.kind == "reasoning" {
                    let channels: &[(&str, &str)] = if block.kind == "message" {
                        &[("content", &block.text)]
                    } else {
                        &[("summary", &block.text), ("content", &block.raw_reasoning)]
                    };
                    for &(key, streamed) in channels {
                        let Some(parts) = item.get(key) else {
                            if !streamed.is_empty() {
                                return Err(decode("completed item omitted streamed text"));
                            }
                            continue;
                        };
                        let mut text = String::new();
                        for part in parts
                            .as_array()
                            .ok_or_else(|| decode("invalid completed content parts"))?
                        {
                            let field = match (block.kind.as_str(), key, string(part, "type")?) {
                                ("message", "content", "output_text")
                                | ("reasoning", "summary", "summary_text")
                                | ("reasoning", "content", "reasoning_text") => "text",
                                ("message", "content", "refusal") => "refusal",
                                _ => return Err(decode("unsupported completed content part")),
                            };
                            text.push_str(string(part, field)?);
                        }
                        if text != streamed {
                            return Err(decode("completed text disagrees with streamed deltas"));
                        }
                    }
                }
                if block.kind == "reasoning" {
                    if block.replay.as_ref().unwrap()["id"] != item["id"] {
                        return Err(decode("completed reasoning item identity changed"));
                    }
                    let mut replay = json!({"type":"reasoning","id":string(item,"id")?,"summary":item.get("summary").cloned().unwrap_or_else(|| json!([]))});
                    for key in ["content", "encrypted_content"] {
                        if let Some(value) = item
                            .get(key)
                            .or_else(|| block.replay.as_ref().and_then(|initial| initial.get(key)))
                        {
                            replay[key] = value.clone();
                        }
                    }
                    block.replay = Some(replay);
                }
                block.closed = true;
                Ok(Vec::new())
            }
            "response.completed" | "response.incomplete" => {
                if self.blocks.values().any(|b| !b.closed) {
                    return Err(incomplete("response terminated with open output items"));
                }
                let response = &value["response"];
                let expected = if kind == "response.completed" {
                    "completed"
                } else {
                    "incomplete"
                };
                if response["status"] != expected {
                    return Err(decode("terminal response status mismatch"));
                }
                self.reason = Some(if kind == "response.incomplete" {
                    match string(&response["incomplete_details"], "reason")? {
                        "max_output_tokens" => StopReason::Length,
                        "content_filter" => StopReason::ContentFilter,
                        other => StopReason::Other(other.into()),
                    }
                } else if self.refused {
                    StopReason::ContentFilter
                } else if self.blocks.values().any(|b| b.kind == "function_call") {
                    StopReason::ToolCalls
                } else {
                    StopReason::Completed
                });
                if let Some(usage) = response.get("usage").filter(|v| !v.is_null()) {
                    self.usage = Some(TokenUsage {
                        input_tokens: usage["input_tokens"]
                            .as_u64()
                            .ok_or_else(|| decode("invalid input usage"))?,
                        output_tokens: usage["output_tokens"]
                            .as_u64()
                            .ok_or_else(|| decode("invalid output usage"))?,
                        cache_hit_tokens: match usage
                            .get("input_tokens_details")
                            .and_then(|v| v.get("cached_tokens"))
                        {
                            Some(v) => v
                                .as_u64()
                                .ok_or_else(|| decode("invalid cached token usage"))?,
                            None => 0,
                        },
                    });
                }
                self.terminal = true;
                Ok(Vec::new())
            }
            "response.function_call_arguments.done" => {
                let block = self
                    .blocks
                    .get(&index(value, "output_index")?)
                    .ok_or_else(|| decode("arguments done for unknown item"))?;
                if block.closed
                    || block.kind != "function_call"
                    || string(value, "arguments")? != block.arguments
                {
                    return Err(decode("completed arguments disagree with deltas"));
                }
                Ok(Vec::new())
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
                    .get(&index(value, "output_index")?)
                    .ok_or_else(|| decode("event for unknown item"))?;
                if block.closed {
                    return Err(decode("event for closed item"));
                }
                Ok(Vec::new())
            }
            _ => Err(decode("unsupported Responses event")),
        }
    }
}

impl ProviderResponseAccumulator for FormatAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ProviderStreamDelta>> {
        self.total = self.total.saturating_add(chunk.len());
        if self.total > 32 * 1024 * 1024 {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "response exceeds 32 MiB",
            ));
        }
        self.buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(i) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=i).collect::<Vec<_>>();
            output.extend(self.line(&line[..line.len() - 1])?);
        }
        Ok(output)
    }
    fn finish(
        mut self: Box<Self>,
    ) -> Result<(ProviderInferenceResponse, Vec<ProviderStreamDelta>)> {
        let mut output = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            output.extend(self.line(&line)?);
        }
        output.extend(self.dispatch()?);
        if !self.terminal || self.reason.is_none() {
            return Err(incomplete("stream ended without a terminal protocol event"));
        }
        let items = self
            .blocks
            .values()
            .filter_map(|block| block.replay.clone())
            .collect::<Vec<_>>();
        let replay =
            (!items.is_empty()).then(|| json!({"api_format":self.format.as_str(),"items":items}));
        replay_items(replay.clone(), self.format, Some(self.reasoning.as_str()))
            .map_err(|_| decode("response contains unusable reasoning replay metadata"))?;
        let mut calls = Vec::new();
        let mut ids = HashSet::new();
        for block in self
            .blocks
            .into_values()
            .filter(|b| b.kind == "tool_use" || b.kind == "function_call")
        {
            if !block.closed
                || block.id.is_empty()
                || block.id.len() > 256
                || block.name.is_empty()
                || block.name.len() > 64
                || !ids.insert(block.id.clone())
                || !serde_json::from_str::<Value>(&block.arguments).is_ok_and(|v| v.is_object())
            {
                return Err(incomplete("invalid completed tool call"));
            }
            calls.push(ToolCall {
                id: block.id,
                tool_name: block.name,
                arguments: block.arguments,
            });
        }
        let reason = self.reason.unwrap();
        if reason == StopReason::ToolCalls && calls.is_empty() {
            return Err(incomplete("tool stop reason has no tool calls"));
        }
        if self.content.is_empty()
            && calls.is_empty()
            && replay.is_none()
            && reason == StopReason::Completed
        {
            return Err(incomplete("completed response has no content"));
        }
        Ok((
            ProviderInferenceResponse {
                replay,
                reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
                content: (!self.content.is_empty()).then_some(self.content),
                tool_calls: calls,
                stop_reason: reason,
                usage: self.usage,
            },
            output,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::inference_request_types::DomainRequestConfig;

    fn request(messages: Vec<AgentMessage>) -> DomainRequest {
        DomainRequest {
            messages,
            tools: vec![],
            config: DomainRequestConfig {
                model: "model".into(),
                temperature: Some(0.5),
                max_output_tokens: Some(100),
                top_p: None,
                stop: vec![],
            },
        }
    }
    fn assistant(calls: Vec<ToolCall>) -> AgentMessage {
        AgentMessage::Assistant {
            reasoning: None,
            replay: None,
            content: None,
            tool_calls: calls,
            usage: None,
            stop_reason: None,
        }
    }
    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            tool_name: "echo".into(),
            arguments: "{}".into(),
        }
    }
    fn tool(id: &str) -> AgentMessage {
        AgentMessage::ToolCall {
            resource_id: None,
            tool_call_id: id.into(),
            tool_name: "echo".into(),
            content: "ok".into(),
            images: vec![],
            failed: false,
        }
    }
    fn frame(value: Value) -> Vec<u8> {
        format!(
            "event: {}\r\ndata: {}\r\n\r\n",
            value["type"].as_str().unwrap(),
            value
        )
        .into_bytes()
    }
    fn feed(
        acc: &mut Box<dyn ProviderResponseAccumulator>,
        events: Vec<Value>,
    ) -> Result<Vec<ProviderStreamDelta>> {
        let mut deltas = Vec::new();
        for event in events {
            for byte in frame(event) {
                deltas.extend(acc.push(&[byte])?);
            }
        }
        Ok(deltas)
    }
    fn message_start() -> Value {
        json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}})
    }
    fn messages_end(reason: &str) -> Vec<Value> {
        vec![
            json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ]
    }

    fn test_images() -> Vec<UserImage> {
        [
            ("image/png", "iVBORw0KGgo="),
            ("image/jpeg", "/9j/"),
            ("image/gif", "R0lGODlh"),
            ("image/webp", "UklGRgAAAABXRUJQ"),
        ]
        .into_iter()
        .map(|(mime, data)| UserImage {
            media_type: mime.into(),
            data: data.into(),
        })
        .collect()
    }

    #[test]
    fn all_formats_preserve_all_image_types_order_and_text_compatibility() {
        for format in [
            ApiFormat::ChatCompletions,
            ApiFormat::Messages,
            ApiFormat::Responses,
        ] {
            for text in ["", "describe"] {
                let user = AgentMessage::User {
                    content: text.into(),
                    images: test_images(),
                };
                let value =
                    serde_json::to_value(format.build_request(request(vec![user])).unwrap())
                        .unwrap();
                let key = if format == ApiFormat::Responses {
                    "input"
                } else {
                    "messages"
                };
                let parts = value[key][0]["content"].as_array().unwrap();
                let offset = usize::from(!text.is_empty());
                assert_eq!(parts.len(), 4 + offset);
                if offset == 1 {
                    assert_eq!(parts[0]["text"], text);
                }
                for (part, image) in parts[offset..].iter().zip(test_images()) {
                    let url = format!("data:{};base64,{}", image.media_type, image.data);
                    match format {
                        ApiFormat::ChatCompletions => {
                            assert_eq!(part, &json!({"type":"image_url","image_url":{"url":url}}))
                        }
                        ApiFormat::Messages => assert_eq!(
                            part,
                            &json!({"type":"image","source":{"type":"base64","media_type":image.media_type,"data":image.data}})
                        ),
                        ApiFormat::Responses => {
                            assert_eq!(part, &json!({"type":"input_image","image_url":url}))
                        }
                    }
                }
            }
            let value = serde_json::to_value(
                format
                    .build_request(request(vec![AgentMessage::User {
                        content: "legacy".into(),
                        images: vec![],
                    }]))
                    .unwrap(),
            )
            .unwrap();
            match format {
                ApiFormat::Messages => assert_eq!(
                    value["messages"][0]["content"],
                    json!([{"type":"text","text":"legacy"}])
                ),
                ApiFormat::ChatCompletions => assert_eq!(value["messages"][0]["content"], "legacy"),
                ApiFormat::Responses => assert_eq!(value["input"][0]["content"], "legacy"),
            }
            let invalid_image = UserImage {
                media_type: "image/jpeg".into(),
                data: "iVBORw0KGgo=".into(),
            };
            let error = format
                .build_request(request(vec![AgentMessage::User {
                    content: String::new(),
                    images: vec![invalid_image],
                }]))
                .unwrap_err();
            assert!(!error.to_string().contains("iVBORw0KGgo="));
        }
    }

    #[test]
    fn encoded_image_context_is_bounded_separately_from_text() {
        let image = UserImage {
            media_type: "image/png".into(),
            data: format!(
                "iVBORw0KGgoA{}",
                "AAAA".repeat((4 * 1024 * 1024 + 512 * 1024 - 9) / 3)
            ),
        };
        let user = AgentMessage::User {
            content: String::new(),
            images: vec![image; 4],
        };
        let (text, encoded) = validated_message_sizes(&user).unwrap();
        assert!(text < 1024);
        assert!(encoded > super::super::MAX_MESSAGES_BYTES);
        let mut single = request(vec![user.clone()]);
        normalize(&mut single).unwrap();
        let mut multiple = request(vec![user.clone(), user]);
        assert!(normalize(&mut multiple).is_err());
    }

    #[test]
    fn tool_image_context_uses_encoded_budget_without_charging_text_budget() {
        let image = UserImage {
            media_type: "image/png".into(),
            data: format!(
                "iVBORw0KGgoA{}",
                "AAAA".repeat((4 * 1024 * 1024 + 512 * 1024 - 9) / 3)
            ),
        };
        let mut result = tool("a");
        if let AgentMessage::ToolCall { images, .. } = &mut result {
            *images = vec![image; 4];
        }
        let (text, encoded) = validated_message_sizes(&result).unwrap();
        assert!(text < 1024);
        assert!(encoded > super::super::MAX_MESSAGES_BYTES);
        let mut single = request(vec![assistant(vec![call("a")]), result.clone()]);
        normalize(&mut single).unwrap();
        let mut second = result.clone();
        if let AgentMessage::ToolCall { tool_call_id, .. } = &mut second {
            *tool_call_id = "b".into();
        }
        let mut multiple = request(vec![assistant(vec![call("a"), call("b")]), result, second]);
        assert!(normalize(&mut multiple).is_err());
    }

    #[test]
    fn format_names_and_stateless_tool_batch() {
        assert_eq!(
            serde_json::to_string(&ApiFormat::ChatCompletions).unwrap(),
            "\"chat_completions\""
        );
        assert!(serde_json::from_str::<ApiFormat>("\"openai\"").is_err());
        let history = vec![
            AgentMessage::System {
                content: "instructions".into(),
            },
            assistant(vec![call("a"), call("b")]),
            tool("b"),
            tool("a"),
        ];
        let value =
            serde_json::to_value(MessagesRequest::try_from(request(history.clone())).unwrap())
                .unwrap();
        assert_eq!(value["system"][0]["text"], "instructions");
        assert_eq!(value["messages"].as_array().unwrap().len(), 2);
        assert_eq!(value["messages"][1]["content"][0]["tool_use_id"], "b");
        let value =
            serde_json::to_value(ResponsesRequest::try_from(request(history.clone())).unwrap())
                .unwrap();
        assert_eq!(value["store"], false);
        assert_eq!(value["input"].as_array().unwrap().len(), 5);
        assert_eq!(value["input"][3]["call_id"], "b");
        let value =
            serde_json::to_value(ChatCompletionsRequest::try_from(request(history)).unwrap())
                .unwrap();
        assert_eq!(value["messages"][1]["tool_calls"][1]["id"], "b");
    }
    #[test]
    fn no_silent_loss_or_invalid_linkage() {
        let mut r = request(vec![AgentMessage::User {
            content: "hi".into(),
            images: vec![],
        }]);
        r.config.stop.push("stop".into());
        assert!(ResponsesRequest::try_from(r).is_err());
        let mut r = request(vec![AgentMessage::User {
            content: "hi".into(),
            images: vec![],
        }]);
        r.config.max_output_tokens = None;
        assert!(MessagesRequest::try_from(r).is_err());
        let a = AgentMessage::Assistant {
            reasoning: Some("private".into()),
            replay: None,
            content: Some("answer".into()),
            tool_calls: vec![],
            usage: None,
            stop_reason: None,
        };
        assert!(MessagesRequest::try_from(request(vec![a.clone()])).is_err());
        assert!(ResponsesRequest::try_from(request(vec![a.clone()])).is_err());
        let chat =
            serde_json::to_value(ChatCompletionsRequest::try_from(request(vec![a])).unwrap())
                .unwrap();
        assert_eq!(chat["messages"][0]["reasoning_content"], "private");
        assert!(ChatCompletionsRequest::try_from(request(vec![
            assistant(vec![call("a")]),
            tool("unknown")
        ]))
        .is_err());
        assert!(MessagesRequest::try_from(request(vec![assistant(vec![call("a")])])).is_err());
    }
    #[test]
    fn messages_text_reasoning_usage_and_terminal() {
        let mut acc = new_messages_accumulator();
        let mut events = vec![
            message_start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"你好"}}),
            json!({"type":"content_block_stop","index":1}),
        ];
        events.extend(messages_end("end_turn"));
        let deltas = feed(&mut acc, events).unwrap();
        assert!(deltas.contains(&ProviderStreamDelta::Content("你好".into())));
        let (response, _) = acc.finish().unwrap();
        assert_eq!(response.reasoning.as_deref(), Some("think"));
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 15,
                output_tokens: 4,
                cache_hit_tokens: 3
            })
        );
        let mut acc = new_messages_accumulator();
        feed(&mut acc, vec![message_start()]).unwrap();
        assert!(acc.finish().is_err());
    }
    fn replay_assistant(r: ProviderInferenceResponse) -> AgentMessage {
        AgentMessage::Assistant {
            reasoning: r.reasoning,
            replay: r.replay,
            content: r.content,
            tool_calls: r.tool_calls,
            usage: r.usage,
            stop_reason: Some(r.stop_reason),
        }
    }
    #[test]
    fn messages_signed_redacted_decode_then_replay() {
        let mut acc = new_messages_accumulator();
        let mut events = vec![
            message_start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"initial ","signature":"prefix-"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thought"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"nature"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque-redacted"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"a","name":"echo","input":{}}}),
            json!({"type":"content_block_stop","index":2}),
        ];
        events.extend(messages_end("tool_use"));
        feed(&mut acc, events).unwrap();
        let history = vec![replay_assistant(acc.finish().unwrap().0), tool("a")];
        let value =
            serde_json::to_value(MessagesRequest::try_from(request(history.clone())).unwrap())
                .unwrap();
        assert_eq!(
            value["messages"][0]["content"][0],
            json!({"type":"thinking","thinking":"initial thought","signature":"prefix-signature"})
        );
        assert_eq!(
            value["messages"][0]["content"][1],
            json!({"type":"redacted_thinking","data":"opaque-redacted"})
        );
        assert_eq!(value["messages"][0]["content"][2]["type"], "tool_use");
        assert!(ResponsesRequest::try_from(request(history.clone())).is_err());
        assert!(ChatCompletionsRequest::try_from(request(history)).is_err());
    }
    #[test]
    fn responses_encrypted_decode_then_replay() {
        let reasoning = json!({"type":"reasoning","id":"rs_replay","summary":[{"type":"summary_text","text":"summary"}],"content":[{"type":"reasoning_text","text":"raw"}],"encrypted_content":"opaque-encrypted"});
        let function = json!({"type":"function_call","call_id":"a","name":"echo","arguments":"{}"});
        let mut acc = new_responses_accumulator();
        feed(&mut acc,vec![response_start(),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_replay","summary":[],"content":[]}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"summary"}),
            json!({"type":"response.reasoning_text.delta","output_index":0,"delta":"raw"}),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
            json!({"type":"response.output_item.added","output_index":1,"item":function}),
            json!({"type":"response.output_item.done","output_index":1,"item":function}),response_end("completed")]).unwrap();
        let history = vec![replay_assistant(acc.finish().unwrap().0), tool("a")];
        let value =
            serde_json::to_value(ResponsesRequest::try_from(request(history.clone())).unwrap())
                .unwrap();
        assert_eq!(value["input"][0], reasoning);
        assert_eq!(value["input"][1], function);
        assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
        assert!(MessagesRequest::try_from(request(history.clone())).is_err());
        assert!(ChatCompletionsRequest::try_from(request(history)).is_err());
    }
    #[test]
    fn malformed_and_oversized_replay_is_rejected() {
        for value in [
            json!({}),
            json!({"api_format":"messages","items":[]}),
            json!({"api_format":"messages","items":[{"type":"thinking","thinking":"unsigned","signature":""}]}),
            json!({"api_format":"messages","items":[{"type":"redacted_thinking","data":"x".repeat(4*1024*1024)}]}),
        ] {
            let mut message = assistant(vec![]);
            if let AgentMessage::Assistant { replay, .. } = &mut message {
                *replay = Some(value);
            }
            assert!(MessagesRequest::try_from(request(vec![message])).is_err());
        }
    }

    #[test]
    fn messages_tool_batch_partial_arguments() {
        let mut acc = new_messages_accumulator();
        let mut events = vec![message_start()];
        for i in 0..2 {
            events.extend([json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("c{i}"),"name":"echo","input":{}}}),json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}),json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":"1}"}}),json!({"type":"content_block_stop","index":i})]);
        }
        events.extend(messages_end("tool_use"));
        feed(&mut acc, events).unwrap();
        let (response, _) = acc.finish().unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[1].arguments, "{\"x\":1}");
        assert_eq!(response.stop_reason, StopReason::ToolCalls);
    }
    #[test]
    fn messages_noncompleted_reasons_and_protocol_errors() {
        for (reason, expected) in [
            ("max_tokens", StopReason::Length),
            ("refusal", StopReason::ContentFilter),
            ("pause_turn", StopReason::Other("pause_turn".into())),
        ] {
            let mut acc = new_messages_accumulator();
            let mut events = vec![message_start()];
            events.extend(messages_end(reason));
            feed(&mut acc, events).unwrap();
            assert_eq!(acc.finish().unwrap().0.stop_reason, expected);
        }
        for event in [
            json!({"type":"error","error":{"message":"secret not forwarded"}}),
            json!({"type":"message_stop"}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}),
        ] {
            let mut acc = new_messages_accumulator();
            assert!(feed(&mut acc, vec![event]).is_err());
        }
        let mut acc = new_messages_accumulator();
        assert!(acc.push(b"data: nope\n\n").is_err());
    }
    fn response_start() -> Value {
        json!({"type":"response.created","response":{"status":"in_progress"}})
    }
    fn response_end(status: &str) -> Value {
        json!({"type":format!("response.{status}"),"response":{"status":status,"usage":{"input_tokens":11,"output_tokens":5,"input_tokens_details":{"cached_tokens":7}},"incomplete_details":{"reason":"max_output_tokens"}}})
    }
    #[test]
    fn responses_tool_batch_and_usage() {
        let mut acc = new_responses_accumulator();
        let mut events = vec![response_start()];
        for i in 0..2 {
            let item = json!({"type":"function_call","call_id":format!("c{i}"),"name":"echo","arguments":""});
            events.push(json!({"type":"response.output_item.added","output_index":i,"item":item}));
            events.push(json!({"type":"response.function_call_arguments.delta","output_index":i,"delta":"{"}));
            events.push(json!({"type":"response.function_call_arguments.delta","output_index":i,"delta":"}"}));
            let mut item = item;
            item["arguments"] = json!("{}");
            events.push(json!({"type":"response.output_item.done","output_index":i,"item":item}));
        }
        events.push(response_end("completed"));
        feed(&mut acc, events).unwrap();
        let (r, _) = acc.finish().unwrap();
        assert_eq!(r.tool_calls.len(), 2);
        assert_eq!(r.stop_reason, StopReason::ToolCalls);
        assert_eq!(r.usage.unwrap().cache_hit_tokens, 7);
    }
    #[test]
    fn responses_text_reasoning_and_incomplete() {
        let mut acc = new_responses_accumulator();
        let events = vec![
            response_start(),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"think"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"think"}]}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message"}}),
            json!({"type":"response.output_text.delta","output_index":1,"delta":"answer"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}}),
            response_end("completed"),
        ];
        feed(&mut acc, events).unwrap();
        let (r, _) = acc.finish().unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("think"));
        assert_eq!(r.content.as_deref(), Some("answer"));
        let mut acc = new_responses_accumulator();
        feed(&mut acc, vec![response_start(), response_end("incomplete")]).unwrap();
        assert_eq!(acc.finish().unwrap().0.stop_reason, StopReason::Length);
    }
    #[test]
    fn responses_raw_reasoning_and_summary_are_validated_separately() {
        for summary in ["", "summary"] {
            let mut acc = new_responses_accumulator();
            let mut events = vec![
                response_start(),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[],"content":[]}}),
                json!({"type":"response.reasoning_text.delta","output_index":0,"delta":"raw"}),
            ];
            if !summary.is_empty() {
                events.push(json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":summary}));
            }
            events.extend([
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","content":[{"type":"reasoning_text","text":"raw"}],"summary":if summary.is_empty() { vec![] } else { vec![json!({"type":"summary_text","text":summary})] }}}),
                json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","content":[]}}),
                json!({"type":"response.output_text.delta","output_index":1,"delta":"answer"}),
                json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}}),
                response_end("completed"),
            ]);
            let deltas = feed(&mut acc, events).unwrap();
            assert!(deltas.contains(&ProviderStreamDelta::Reasoning("raw".into())));
            let response = acc.finish().unwrap().0;
            assert_eq!(response.reasoning, Some(format!("raw{summary}")));
            assert_eq!(response.content.as_deref(), Some("answer"));
        }

        for (raw, summary) in [("wrong", "summary"), ("raw", "wrong")] {
            let mut acc = new_responses_accumulator();
            assert!(feed(&mut acc, vec![
                response_start(),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
                json!({"type":"response.reasoning_text.delta","output_index":0,"delta":"raw"}),
                json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"summary"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","content":[{"type":"reasoning_text","text":raw}],"summary":[{"type":"summary_text","text":summary}]}}),
            ]).is_err());
        }
    }

    #[test]
    fn responses_require_terminal_closed_items_and_valid_arguments() {
        let mut acc = new_responses_accumulator();
        feed(&mut acc, vec![response_start()]).unwrap();
        assert!(acc.finish().is_err());
        let mut acc = new_responses_accumulator();
        assert!(feed(
            &mut acc,
            vec![response_start(), json!({"type":"response.failed"})]
        )
        .is_err());
        let mut acc = new_responses_accumulator();
        assert!(feed(&mut acc,vec![response_start(),json!({"type":"response.output_item.added","output_index":999999,"item":{"type":"message"}})]).is_err());
        let mut acc = new_responses_accumulator();
        assert!(feed(&mut acc,vec![response_start(),json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),response_end("completed")]).is_err());
        let item = json!({"type":"function_call","call_id":"c","name":"echo","arguments":"{"});
        let mut acc = new_responses_accumulator();
        feed(
            &mut acc,
            vec![
                response_start(),
                json!({"type":"response.output_item.added","output_index":0,"item":item}),
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
                response_end("completed"),
            ],
        )
        .unwrap();
        assert!(acc.finish().is_err());
    }
    #[test]
    fn terminal_is_final_and_response_size_is_bounded() {
        let mut acc = new_messages_accumulator();
        let mut events = vec![message_start()];
        events.extend(messages_end("max_tokens"));
        feed(&mut acc, events).unwrap();
        assert!(feed(&mut acc, vec![json!({"type":"ping"})]).is_err());
        let mut acc = new_responses_accumulator();
        assert_eq!(
            acc.push(&vec![b'x'; 32 * 1024 * 1024 + 1])
                .unwrap_err()
                .kind(),
            InferenceErrorKind::ResponseTooLarge
        );
        let mut acc = new_responses_accumulator();
        assert!(feed(&mut acc,vec![response_start(),json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),json!({"type":"response.output_text.delta","output_index":0,"delta":"streamed"}),json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"different"}]}})]).is_err());
    }
}
