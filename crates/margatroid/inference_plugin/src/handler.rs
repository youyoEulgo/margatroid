use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::{Entity, World};
use futures_util::StreamExt;
use margatroid_types::agent_components::{Agent, AgentState};
use margatroid_types::client_components::WorldClientExt;
use margatroid_types::execution_components::{
    ExecutionActivity, ExecutionActivityKind, ExecutionError,
};
use margatroid_types::inference_events::InferenceReply;
use margatroid_types::message_types::{AgentMessage, ToolDefinition};
use margatroid_types::resource_components::{
    CallableDefinitionSnapshot, CallableResource, ToolResource,
};
use reqwest::Url;
use resource_id_plugin::{ResourceID, ResourceSet};
use serde::Serialize;
use server_plugin::{WebSocketMessage, WebSocketMessageSender, WebSocketSender};
use tokio_util::sync::CancellationToken;

use crate::types::*;
use crate::{InferenceConfig, InferenceError, InferenceErrorKind};
use margatroid_types::inference_events::{
    InferenceReloadOutput as ReloadOutput, InferenceRequest, ReloadModelRoutes,
    ReloadModelRoutesResult,
};
use margatroid_types::message_types::InferenceResponse;

fn parse_context_window(value: Option<&str>) -> Result<u64, InferenceError> {
    let value = value.unwrap_or("1m").trim().to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = value.strip_suffix('k') {
        (number, 1_000_u64)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 1_000_000_u64)
    } else if let Some(number) = value.strip_suffix('b') {
        (number, 1_000_000_000_u64)
    } else if let Some(number) = value.strip_suffix('t') {
        (number, 1_000_000_000_000_u64)
    } else {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidModelRoute,
            "context_window must use k, m, b, or t suffix",
        ));
    };
    let count = number.parse::<u64>().map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::InvalidModelRoute,
            "context_window amount is invalid",
        )
    })?;
    count
        .checked_mul(multiplier)
        .filter(|value| *value > 0 && *value <= i64::MAX as u64)
        .ok_or_else(|| {
            InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "context_window must be positive and fit a Lua integer",
            )
        })
}

pub(crate) fn default_config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".margatroid")
        .join("models.toml")
}

pub(crate) fn load_model_routes(
    path: &Path,
    factories: &HashMap<String, ErasedProviderAdapterFactory>,
) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError> {
    let source = read_model_source(path)?;
    compile_model_routes(&source, factories)
}

fn read_model_source(path: &Path) -> Result<String, InferenceError> {
    let mut source = String::new();
    fs::File::open(path)
        .and_then(|file| {
            file.take((MAX_CONFIG_BYTES + 1) as u64)
                .read_to_string(&mut source)
        })
        .map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::ConfigReadFailed,
                "model route configuration could not be read",
            )
        })?;
    if source.len() > MAX_CONFIG_BYTES {
        return Err(InferenceError::new(
            InferenceErrorKind::ConfigReadFailed,
            "model route configuration exceeds the size limit",
        ));
    }
    toml::from_str::<ModelRouteDocument>(&source).map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::ConfigDecodeFailed,
            "model route configuration could not be decoded",
        )
    })?;
    Ok(source)
}

fn compile_model_routes(
    source: &str,
    factories: &HashMap<String, ErasedProviderAdapterFactory>,
) -> Result<HashMap<ModelId, ConfiguredModelRoute>, InferenceError> {
    let document = toml::from_str::<ModelRouteDocument>(source).map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::ConfigDecodeFailed,
            "model route configuration could not be decoded",
        )
    })?;
    if document.models.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidModelRoute,
            "model route configuration is empty",
        ));
    }
    let mut routes = HashMap::new();
    for config in document.models {
        if config.model.is_empty()
            || config.api_key.is_empty()
            || config.api_type.is_empty()
            || config.base_url.is_empty()
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "model route contains an empty required field",
            ));
        }
        let id = ModelId::new(config.id)?;
        if routes.contains_key(&id) {
            return Err(InferenceError::new(
                InferenceErrorKind::DuplicateModelId,
                format!("model route `{id}` is duplicated"),
            ));
        }
        let base_url = Url::parse(&config.base_url).map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "model route base URL is invalid",
            )
        })?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host().is_none() {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "model route base URL must use http or https",
            ));
        }
        let factory = factories.get(&config.api_type).ok_or_else(|| {
            InferenceError::new(
                InferenceErrorKind::UnsupportedApiType,
                "model route api type is not registered",
            )
        })?;
        let context_window_tokens = parse_context_window(config.context_window.as_deref())?;
        let adapter = factory.build(ProviderRouteInput {
            provider: config.provider.as_deref(),
            base_url: &base_url,
            api_key: &config.api_key,
            thinking: config.thinking.as_deref(),
            reasoning_effort: config.reasoning_effort.as_deref(),
        })?;
        let runtime_info = margatroid_types::model_components::ModelRuntimeInfo {
            id: id.as_str().to_owned(),
            model: config.model.clone(),
            api_type: config.api_type,
            provider: config.provider,
            context_window_tokens,
            thinking: config.thinking,
            reasoning_effort: config.reasoning_effort,
        };
        routes.insert(
            id,
            ConfiguredModelRoute {
                model: config.model,
                context_window_tokens,
                adapter,
                runtime_info,
            },
        );
    }
    Ok(routes)
}

async fn send_provider_request(
    client: &reqwest::Client,
    request: ProviderHttpRequest,
) -> Result<reqwest::Response, InferenceError> {
    let endpoint = safe_endpoint(&request.url);
    let mut outbound = reqwest::Request::new(request.method, request.url);
    *outbound.headers_mut() = request.headers;
    *outbound.body_mut() = Some(reqwest::Body::from(request.body));
    client.execute(outbound).await.map_err(|error| {
        InferenceError::new(
            InferenceErrorKind::RequestFailed,
            format!(
                "inference provider request failed at {endpoint}: {}",
                summarize_reqwest_error(&error)
            ),
        )
    })
}

fn safe_endpoint(url: &Url) -> String {
    url.origin().ascii_serialization()
}
pub(crate) fn summarize_reqwest_error(error: &reqwest::Error) -> String {
    let category = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_body() {
        "response body failed"
    } else if error.is_decode() {
        "response decoding failed"
    } else if error.is_request() {
        "request construction failed"
    } else {
        "HTTP transport failed"
    };
    category.into()
}
fn provider_error_detail(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        for pointer in ["/error/message", "/error", "/message", "/detail"] {
            if let Some(message) = value.pointer(pointer).and_then(|value| value.as_str()) {
                let message = single_line(message);
                if !message.is_empty() {
                    return Some(message);
                }
            }
        }
    }
    let message = single_line(&String::from_utf8_lossy(body));
    (!message.is_empty()).then_some(message)
}
fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
async fn read_bounded_body(response: reqwest::Response, limit: usize) -> Vec<u8> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        let remaining = limit.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() >= limit {
            break;
        }
    }
    body
}

struct PreparedInference {
    #[cfg(test)]
    terminal_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    id: String,
    agent_id: ResourceID,
    agent_instance: String,
    client: reqwest::Client,
    request: ProviderHttpRequest,
    adapter: ErasedProviderAdapter,
    tool_names: HashSet<String>,
    senders: Vec<WebSocketSender>,
    cancel: CancellationToken,
    reply: InferenceReply,
}

fn complete(reply: &InferenceReply, result: Result<InferenceResponse, InferenceError>) {
    if let Some(sender) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = sender.send(result);
    }
}

struct TaskGuard {
    activity: ExecutionActivity,
    reply: InferenceReply,
    finished: bool,
    started: bool,
}
impl Drop for TaskGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.started {
            self.activity
                .mark_cleanup_failed(ExecutionError::CleanupFailed {
                    activity_id: self.activity.id,
                    message: "inference task dropped before cleanup proof".into(),
                });
        } else if let Err(error) = self.activity.rollback_submission() {
            self.activity.mark_cleanup_failed(error);
        }
        complete(
            &self.reply,
            Err(InferenceError::new(
                if self.started {
                    InferenceErrorKind::TaskAborted
                } else {
                    InferenceErrorKind::SubmitFailed
                },
                "inference task did not finish",
            )),
        );
    }
}

pub(crate) fn handle_inference(world: &mut World, mut request: InferenceRequest) {
    let sender = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let Some(sender) = sender else {
        return;
    };
    request.reply = Arc::new(Mutex::new(Some(sender)));
    let reply = request.reply.clone();
    if world
        .get_component::<Agent>(request.agent)
        .is_some_and(|agent| {
            !matches!(agent.state, AgentState::Working)
                || request.turn.is_none()
                || request.turn != agent.turn
        })
    {
        complete(&reply, Ok(InferenceResponse::aborted()));
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare_inference(world, request)
    }))
    .unwrap_or_else(|_| {
        Err(InferenceError::new(
            InferenceErrorKind::TaskPanicked,
            "inference preparation panicked",
        ))
    });
    let (prepared, activity) = match result {
        Ok(value) => value,
        Err(error) => {
            complete(&reply, Err(error));
            return;
        }
    };
    let schedule = world
        .get_resource::<InferenceConfig>()
        .unwrap()
        .schedule
        .clone();
    let submit_reply = reply.clone();
    let guard = TaskGuard {
        activity,
        reply,
        finished: false,
        started: false,
    };
    let submit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || async move {
            execute_inference(prepared, guard).await;
            Ok::<(), AsyncTaskError>(())
        });
    }));
    if submit.is_err() {
        complete(
            &submit_reply,
            Err(InferenceError::new(
                InferenceErrorKind::SubmitFailed,
                "inference submission failed",
            )),
        );
    }
}

fn prepare_inference(
    world: &World,
    request: InferenceRequest,
) -> Result<(PreparedInference, ExecutionActivity), InferenceError> {
    if request.id.trim().is_empty()
        || request.id.len() > 256
        || request.id.chars().any(char::is_control)
    {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidRequest,
            "invalid inference request ID",
        ));
    }
    if request.cancel.is_cancelled() {
        return Err(InferenceError::new(
            InferenceErrorKind::Cancelled,
            "inference cancelled before preparation",
        ));
    }
    if !world.is_alive(request.agent) {
        return Err(InferenceError::new(
            InferenceErrorKind::AgentNotAlive,
            "agent instance is not alive",
        ));
    }
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        InferenceError::new(InferenceErrorKind::AgentMissing, "agent component missing")
    })?;
    let config = world.get_resource::<InferenceConfig>().ok_or_else(|| {
        InferenceError::new(
            InferenceErrorKind::PreparationFailed,
            "inference plugin missing",
        )
    })?;
    let model = ModelId::new(agent.info.model.model.clone())?;
    let route = config.routes.get(&model).ok_or_else(|| {
        InferenceError::new(
            InferenceErrorKind::ModelRouteNotFound,
            "model route not found",
        )
    })?;
    let parameters = InferenceParameters::new(
        agent.info.model.temperature,
        agent.info.model.max_output_tokens,
        agent.info.model.top_p,
        agent.info.model.stop.clone(),
    );
    parameters.validate()?;
    let tools = collect_tool_definitions(world, request.agent, &request.tools)?;
    let messages = normalize_messages(request.messages)?;
    let encoded = route.adapter.build_request(ProviderInput::new(
        route.model(),
        &parameters,
        &messages,
        &tools,
    ))?;
    let mut seen = HashSet::new();
    let senders = world
        .client_webui_senders()
        .into_iter()
        .filter(|s| !s.is_closed() && seen.insert(s.connection_id()))
        .collect();
    let activity = agent
        .execution
        .register_for_turn(ExecutionActivityKind::Inference, request.turn.clone())
        .map_err(|e| InferenceError::new(InferenceErrorKind::Cancelled, e.to_string()))?;

    let prepared = PreparedInference {
        #[cfg(test)]
        terminal_gate: None,
        id: request.id,
        agent_id: agent.resourceid.clone(),
        agent_instance: format!("{:?}", request.agent),
        client: config.client.clone(),
        request: encoded,
        adapter: route.adapter.clone(),
        tool_names: tools.into_iter().map(|t| t.name).collect(),
        senders,
        cancel: request.cancel,
        reply: request.reply,
    };
    Ok((prepared, activity))
}

fn collect_tool_definitions(
    world: &World,
    agent: Entity,
    visible: &[CallableResource],
) -> Result<Vec<ToolDefinition>, InferenceError> {
    let mut tools = Vec::new();
    for selected in visible {
        validate_owner(world, agent, &selected.resource_id)?;
        let (description, input_schema) =
            match (&*selected.definition, selected.resource_id.resource_type()) {
                (CallableDefinitionSnapshot::Tool(resource), "tool") => {
                    (resource.description.clone(), resource.input_schema.clone())
                }
                _ => {
                    return Err(InferenceError::new(
                        InferenceErrorKind::UnsupportedResource,
                        "snapshot kind does not match resource identity",
                    ))
                }
            };
        tools.push(ToolDefinition {
            name: selected.alias.clone(),
            description,
            input_schema,
        });
    }
    validate_tools(&tools)?;
    Ok(tools)
}

fn validate_owner(world: &World, agent: Entity, id: &ResourceID) -> Result<(), InferenceError> {
    let present = match id.resource_type() {
        "tool" => world
            .get_component::<ResourceSet<ToolResource>>(agent)
            .is_some_and(|set| set.get(id).is_some()),
        _ => {
            return Err(InferenceError::new(
                InferenceErrorKind::UnsupportedResource,
                "resource is not callable",
            ))
        }
    };
    if !present {
        return Err(InferenceError::new(
            InferenceErrorKind::ResourceMissing,
            "resource not loaded on target agent",
        ));
    }
    let owner = world.get_component::<Agent>(agent).ok_or_else(|| {
        InferenceError::new(
            InferenceErrorKind::ResourceMissing,
            "resource owner missing",
        )
    })?;
    if !owner.info.image_dependencies.contains(id)
        || !owner
            .import_bindings
            .contains_resource(id, margatroid_types::block_types::InnerType::Tool)
    {
        return Err(InferenceError::new(
            InferenceErrorKind::ResourceMissing,
            "resource is not imported or permitted for target agent",
        ));
    }
    Ok(())
}

fn has_external_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            (key == "$ref"
                && value
                    .as_str()
                    .is_some_and(|reference| !reference.starts_with('#')))
                || has_external_reference(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(has_external_reference),
        _ => false,
    }
}

fn validate_tools(tools: &[ToolDefinition]) -> Result<(), InferenceError> {
    if tools.len() > MAX_TOOL_COUNT {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidToolDefinitions,
            "too many tools",
        ));
    }
    let mut names = HashSet::new();
    for tool in tools {
        if tool.name.is_empty()
            || tool.name.len() > 64
            || !tool
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || tool.description.trim().is_empty()
            || tool.description.len() > MAX_TOOL_DESCRIPTION_BYTES
            || !tool.input_schema.is_object()
            || tool.input_schema.get("type").and_then(|v| v.as_str()) != Some("object")
            || has_external_reference(&tool.input_schema)
            || jsonschema::validator_for(&tool.input_schema).is_err()
            || !names.insert(tool.name.clone())
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidToolDefinitions,
                "invalid or duplicate tool definition",
            ));
        }
    }
    Ok(())
}

fn normalize_messages(messages: Vec<AgentMessage>) -> Result<Vec<ProviderMessage>, InferenceError> {
    fn expand(
        messages: Vec<AgentMessage>,
        depth: usize,
        out: &mut Vec<ProviderMessage>,
        bytes: &mut usize,
    ) -> Result<(), InferenceError> {
        if depth > 32 {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "inject nesting too deep",
            ));
        }
        for message in messages {
            if matches!(
                &message,
                AgentMessage::Assistant {
                    stop_reason: Some(margatroid_types::message_types::StopReason::Cancelled),
                    ..
                }
            ) {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "cancelled assistant cannot enter inference context",
                ));
            }
            if let AgentMessage::Inject { messages } = message {
                expand(messages, depth + 1, out, bytes)?;
                continue;
            }
            let size = serde_json::to_vec(&message)
                .map_err(|_| {
                    InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "message cannot be encoded",
                    )
                })?
                .len();
            *bytes = bytes.saturating_add(size);
            if size > MAX_MESSAGE_BYTES || *bytes > MAX_MESSAGES_BYTES || out.len() >= 4096 {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "message size or count exceeds limit",
                ));
            }
            out.push(match message {
                AgentMessage::System { content } => ProviderMessage::System { content },
                AgentMessage::User { content } => ProviderMessage::User { content },
                AgentMessage::Error { message } => ProviderMessage::Error { message },
                AgentMessage::Assistant {
                    reasoning,
                    content,
                    tool_calls,
                    ..
                } => ProviderMessage::Assistant {
                    reasoning,
                    content,
                    tool_calls,
                },
                AgentMessage::ToolCall {
                    tool_call_id,
                    content,
                    ..
                } => ProviderMessage::Tool {
                    tool_call_id,
                    content,
                },
                AgentMessage::Inject { .. } => unreachable!(),
            });
        }
        Ok(())
    }
    let mut out = Vec::new();
    expand(messages, 0, &mut out, &mut 0)?;
    if out.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidMessages,
            "empty inference messages",
        ));
    }
    let mut pending = HashSet::new();
    let mut ids = HashSet::new();
    for message in &out {
        match message {
            ProviderMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                if !pending.is_empty() || (content.is_none() && tool_calls.is_empty()) {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "invalid assistant tool history",
                    ));
                }
                for call in tool_calls {
                    if call.id.is_empty()
                        || call.tool_name.is_empty()
                        || !ids.insert(call.id.clone())
                        || !serde_json::from_str::<serde_json::Value>(&call.arguments)
                            .is_ok_and(|v| v.is_object())
                    {
                        return Err(InferenceError::new(
                            InferenceErrorKind::InvalidMessages,
                            "invalid assistant tool call",
                        ));
                    }
                    pending.insert(call.id.clone());
                }
            }
            ProviderMessage::Tool { tool_call_id, .. } => {
                if !pending.remove(tool_call_id) {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "orphan or duplicate tool result",
                    ));
                }
            }
            _ if !pending.is_empty() => {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "tool calls lack adjacent results",
                ))
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidMessages,
            "tool calls lack results",
        ));
    }
    Ok(out)
}

async fn receiver_closed(reply: &InferenceReply) {
    loop {
        if reply
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|sender| sender.is_closed())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn execute_inference(prepared: PreparedInference, guard: TaskGuard) {
    let owner_cancel = CancellationToken::new();
    let owner_drop = owner_cancel.clone().drop_guard();
    let supervisor = tokio::spawn(supervise_inference(prepared, guard, owner_cancel));
    let _ = supervisor.await;
    owner_drop.disarm();
}

async fn supervise_inference(
    prepared: PreparedInference,
    mut guard: TaskGuard,
    owner_cancel: CancellationToken,
) {
    if let Err(error) = guard.activity.mark_started() {
        complete(
            &guard.reply,
            Err(InferenceError::new(
                InferenceErrorKind::ExecutionFailed,
                error.to_string(),
            )),
        );
        return;
    }
    guard.started = true;
    let reply = prepared.reply.clone();
    let cancel = prepared.cancel.clone();
    let child = guard.activity.cancel.clone();
    let turn_cancel = guard.activity.turn_cancel.clone();
    let terminal_senders = prepared.senders.clone();
    let terminal_id = prepared.id.clone();
    let terminal_agent = prepared.agent_id.clone();
    let terminal_instance = prepared.agent_instance.clone();
    #[cfg(test)]
    let terminal_gate = prepared.terminal_gate.clone();
    let mut worker = tokio::spawn(run_provider(prepared));
    let mut joined = false;
    let mut result = tokio::select! {
        biased;
        _ = owner_cancel.cancelled() => Err(InferenceError::new(InferenceErrorKind::TaskAborted, "inference owner aborted")),
        _ = child.cancelled() => Err(InferenceError::new(InferenceErrorKind::Cancelled, "agent execution cancelled")),
        _ = cancel.cancelled() => Err(InferenceError::new(InferenceErrorKind::Cancelled, "inference request cancelled")),
        _ = turn_cancel.cancelled() => Ok(InferenceResponse::aborted()),
        _ = receiver_closed(&reply) => Err(InferenceError::new(InferenceErrorKind::ResponseLost, "inference receiver closed")),
        _ = tokio::time::sleep(Duration::from_secs(180)) => Err(InferenceError::new(InferenceErrorKind::Timeout, "inference request timed out")),
        result = &mut worker => {
            joined = true;
            result.unwrap_or_else(|error| Err(InferenceError::new(if error.is_panic() { InferenceErrorKind::TaskPanicked } else { InferenceErrorKind::TaskAborted }, "inference HTTP worker stopped")))
        },
    };
    if !joined {
        worker.abort();
        let _ = worker.await;
    }
    {
        let terminal = async {
            #[cfg(test)]
            if let Some((entered, release)) = terminal_gate {
                entered.notify_one();
                release.notified().await;
            }
            push_finished(
                terminal_senders,
                &terminal_id,
                &terminal_agent,
                &terminal_instance,
                result.as_ref().err(),
            )
            .await;
        };
        tokio::pin!(terminal);
        let terminal_aborted = tokio::select! {
            biased;
            _ = owner_cancel.cancelled() => true,
            _ = &mut terminal => false,
        };
        if terminal_aborted {
            drop(terminal);
        }
    }
    if owner_cancel.is_cancelled() {
        result = Err(InferenceError::new(
            InferenceErrorKind::TaskAborted,
            "inference owner aborted",
        ));
    }
    match guard.activity.mark_completed() {
        Ok(()) => {
            guard.finished = true;
            complete(&reply, result);
        }
        Err(error) => {
            guard.activity.mark_cleanup_failed(error.clone());
            complete(
                &reply,
                Err(InferenceError::new(
                    InferenceErrorKind::CleanupFailed,
                    error.to_string(),
                )),
            );
        }
    }
}

async fn run_provider(
    mut prepared: PreparedInference,
) -> Result<InferenceResponse, InferenceError> {
    let response = send_provider_request(&prepared.client, prepared.request).await?;
    let status = response.status();
    if !status.is_success() {
        let body = read_bounded_body(response, MAX_ERROR_BODY_BYTES).await;
        return Err(InferenceError::with_status(
            InferenceErrorKind::ResponseStatus,
            Some(status.as_u16()),
            provider_error_detail(&body).unwrap_or_else(|| "provider rejected request".into()),
        ));
    }
    let mut accumulator = prepared
        .adapter
        .begin_response(status, response.headers())?;
    let mut body = response.bytes_stream();
    let mut bytes = 0usize;
    let mut sequence = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "provider stream disconnected",
            )
        })?;
        bytes = bytes.saturating_add(chunk.len());
        if bytes > 32 * 1024 * 1024 {
            return Err(InferenceError::new(
                InferenceErrorKind::ResponseTooLarge,
                "provider response exceeds limit",
            ));
        }
        for delta in accumulator.push(&chunk)? {
            sequence += 1;
            push_stream(
                &mut prepared.senders,
                &prepared.id,
                &prepared.agent_instance,
                &prepared.agent_id,
                sequence,
                delta,
            )
            .await;
        }
    }
    let (response, deltas) = accumulator.finish()?;
    if response
        .tool_calls
        .iter()
        .any(|call| !prepared.tool_names.contains(&call.tool_name))
    {
        return Err(InferenceError::new(
            InferenceErrorKind::ResponseDecodeFailed,
            "provider returned an unknown tool name",
        ));
    }
    for delta in deltas {
        sequence += 1;
        push_stream(
            &mut prepared.senders,
            &prepared.id,
            &prepared.agent_instance,
            &prepared.agent_id,
            sequence,
            delta,
        )
        .await;
    }
    Ok(InferenceResponse {
        message: AgentMessage::Assistant {
            reasoning: response.reasoning,
            content: response.content,
            tool_calls: response.tool_calls,
            usage: response.usage,
            stop_reason: Some(response.stop_reason.clone()),
        },
        stop_reason: response.stop_reason,
    })
}

#[derive(Serialize)]
struct DeltaFrame<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    id: &'a str,
    agent: &'a ResourceID,
    agent_instance: &'a str,
    sequence: u64,
    content: &'a str,
}
async fn push_stream(
    senders: &mut Vec<WebSocketSender>,
    id: &str,
    agent_instance: &str,
    agent: &ResourceID,
    sequence: u64,
    delta: ProviderStreamDelta,
) {
    let (message_type, content) = match &delta {
        ProviderStreamDelta::Reasoning(content) => ("agent.message.reasoning_delta", content),
        ProviderStreamDelta::Content(content) => ("agent.message.delta", content),
    };
    let Ok(encoded) = serde_json::to_string(&DeltaFrame {
        message_type,
        id,
        agent,
        agent_instance,
        sequence,
        content,
    }) else {
        return;
    };
    let results = futures_util::future::join_all(senders.iter().map(|sender| {
        let encoded = encoded.clone();
        async move {
            if sender.is_closed() {
                return false;
            }
            tokio::time::timeout(
                Duration::from_millis(100),
                WebSocketMessageSender::new(
                    vec![sender.clone()],
                    WebSocketMessage::Text(encoded.into()),
                )
                .send(),
            )
            .await
            .is_ok()
        }
    }))
    .await;
    let mut index = 0;
    senders.retain(|sender| {
        let keep = results[index] && !sender.is_closed();
        index += 1;
        keep
    });
}

#[derive(Serialize)]
struct FinishedError<'a> {
    kind: String,
    message: &'a str,
}
#[derive(Serialize)]
struct FinishedFrame<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    id: &'a str,
    agent: &'a ResourceID,
    agent_instance: &'a str,
    error: Option<FinishedError<'a>>,
}
async fn push_finished(
    senders: Vec<WebSocketSender>,
    id: &str,
    agent: &ResourceID,
    agent_instance: &str,
    error: Option<&InferenceError>,
) {
    let frame = FinishedFrame {
        message_type: "agent.message.finished",
        id,
        agent,
        agent_instance,
        error: error.map(|e| FinishedError {
            kind: format!("{:?}", e.kind),
            message: &e.message,
        }),
    };
    let Ok(encoded) = serde_json::to_string(&frame) else {
        return;
    };
    futures_util::future::join_all(
        senders
            .into_iter()
            .filter(|s| !s.is_closed())
            .map(|sender| {
                let encoded = encoded.clone();
                async move {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(100),
                        WebSocketMessageSender::new(
                            vec![sender],
                            WebSocketMessage::Text(encoded.into()),
                        )
                        .send(),
                    )
                    .await;
                }
            }),
    )
    .await;
}

struct ReloadGuard {
    id: String,
    events: app_runtime_plugin::RuntimeEventSender,
    finished: bool,
}
impl ReloadGuard {
    fn complete(mut self, result: Result<String, InferenceError>) {
        self.events.send_event(ReloadOutput {
            id: self.id.clone(),
            result,
        });
        self.finished = true;
    }
}
impl Drop for ReloadGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.events.send_event(ReloadOutput {
                id: self.id.clone(),
                result: Err(InferenceError::new(
                    InferenceErrorKind::TaskAborted,
                    "model reload task aborted",
                )),
            });
        }
    }
}
pub(crate) fn handle_reload_model_routes(world: &mut World, request: ReloadModelRoutes) {
    let config = world.get_resource_mut::<InferenceConfig>().unwrap();
    let error = if request.id.trim().is_empty()
        || request.id.len() > 256
        || request.id.chars().any(char::is_control)
    {
        Some(InferenceError::new(
            InferenceErrorKind::InvalidRequest,
            "invalid reload ID",
        ))
    } else if config.reload_id.is_some() {
        Some(InferenceError::new(
            InferenceErrorKind::ConfigBusy,
            "model reload already running",
        ))
    } else {
        None
    };
    if let Some(error) = error {
        world.send_event(ReloadModelRoutesResult {
            id: request.id,
            result: Err(error),
        });
        return;
    }
    config.reload_id = Some(request.id.clone());
    let path = config.path.clone();
    let schedule = config.schedule.clone();
    let guard = ReloadGuard {
        id: request.id,
        events: world.event_sender(),
        finished: false,
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || async move {
            let result = tokio::task::spawn_blocking(move || read_model_source(&path))
                .await
                .unwrap_or_else(|_| {
                    Err(InferenceError::new(
                        InferenceErrorKind::TaskPanicked,
                        "model reload task panicked",
                    ))
                });
            guard.complete(result);
            Ok::<(), AsyncTaskError>(())
        })
    }));
}
pub(crate) fn handle_model_routes_reloaded(world: &mut World, output: ReloadOutput) {
    let config = world.get_resource_mut::<InferenceConfig>().unwrap();
    if config.reload_id.as_deref() != Some(&output.id) {
        return;
    }
    config.reload_id = None;
    let result = output
        .result
        .and_then(|source| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compile_model_routes(&source, &config.factories)
            }))
            .unwrap_or_else(|_| {
                Err(InferenceError::new(
                    InferenceErrorKind::TaskPanicked,
                    "model route compilation panicked",
                ))
            })
        })
        .map(|routes| {
            let count = routes.len();
            config.routes = routes;
            count
        });
    if result.is_ok() {
        let catalog = crate::runtime_catalog(&config.routes);
        world.insert_resource(catalog);
    }
    world.send_event(ReloadModelRoutesResult {
        id: output.id,
        result,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::execution_components::AgentExecutionScope;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    async fn mock(body: &'static str, hold: bool) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 65536];
            socket.read(&mut request).await.unwrap();
            let header = if hold {
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                    .to_owned()
            } else {
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length:{}\r\nConnection: close\r\n\r\n", body.len())
            };
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            if hold {
                let mut byte = [0];
                let _ = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte)).await;
            }
        });
        (url, task)
    }

    fn prepared(
        url: &Url,
        deepseek: bool,
    ) -> (
        PreparedInference,
        TaskGuard,
        oneshot::Receiver<Result<InferenceResponse, InferenceError>>,
        AgentExecutionScope,
    ) {
        let adapter = if deepseek {
            DeepSeekAdapterFactory
                .build(ProviderRouteInput {
                    provider: None,
                    base_url: url,
                    api_key: "test",
                    thinking: Some("disabled"),
                    reasoning_effort: None,
                })
                .unwrap()
        } else {
            OpenAiAdapterFactory
                .build(ProviderRouteInput {
                    provider: None,
                    base_url: url,
                    api_key: "test",
                    thinking: None,
                    reasoning_effort: None,
                })
                .unwrap()
        };
        let request = adapter
            .build_request(ProviderInput::new(
                "mock",
                &InferenceParameters::default(),
                &[ProviderMessage::User {
                    content: "hi".into(),
                }],
                &[],
            ))
            .unwrap();
        let (tx, rx) = oneshot::channel();
        let reply = Arc::new(Mutex::new(Some(tx)));
        let scope = AgentExecutionScope::new();
        let activity = scope.register(ExecutionActivityKind::Inference).unwrap();
        let guard = TaskGuard {
            activity,
            reply: reply.clone(),
            started: false,
            finished: false,
        };
        (
            PreparedInference {
                terminal_gate: None,
                id: "mock".into(),
                agent_id: ResourceID::parse("agent:test/main").unwrap(),
                agent_instance: "opaque".into(),
                client: reqwest::Client::new(),
                request,
                adapter,
                tool_names: HashSet::from(["echo".into()]),
                senders: vec![],
                cancel: CancellationToken::new(),
                reply,
            },
            guard,
            rx,
            scope,
        )
    }

    #[tokio::test]
    async fn mock_http_preserves_reasoning_tool_fragments_usage_and_length() {
        let (url, server) = mock("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\",\"tool_calls\":[{\"index\":0,\"id\":\"call\",\"function\":{\"name\":\"echo\",\"arguments\":\"{\"}}]},\"finish_reason\":null}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]},\"finish_reason\":\"length\"}]}\n\ndata: {\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"prompt_cache_hit_tokens\":3}}\n\ndata: [DONE]\n\n", false).await;
        let (prepared, guard, rx, scope) = prepared(&url, true);
        execute_inference(prepared, guard).await;
        let response = rx.await.unwrap().unwrap();
        assert_eq!(response.stop_reason, StopReason::Length);
        let AgentMessage::Assistant {
            reasoning,
            tool_calls,
            usage,
            stop_reason,
            ..
        } = response.message
        else {
            panic!()
        };
        assert_eq!(stop_reason, Some(response.stop_reason));
        assert_eq!(reasoning.as_deref(), Some("think"));
        assert_eq!(tool_calls[0].arguments, "{}");
        assert_eq!(usage.unwrap().cache_hit_tokens, 3);
        assert_eq!(scope.activity_count(), 0);
        server.await.unwrap();
    }

    struct PanickingAdapter;
    impl ProviderAdapter for PanickingAdapter {
        fn build_request(
            &self,
            _: ProviderInput<'_>,
        ) -> Result<ProviderHttpRequest, InferenceError> {
            panic!("encode panic")
        }
        fn begin_response(
            &self,
            _: reqwest::StatusCode,
            _: &reqwest::header::HeaderMap,
        ) -> Result<Box<dyn ProviderResponseAccumulator>, InferenceError> {
            panic!("decode panic")
        }
    }

    #[tokio::test]
    async fn provider_panic_still_completes_reply_after_activity_cleanup() {
        let (url, server) = mock("", false).await;
        let (mut prepared, guard, rx, scope) = prepared(&url, false);
        prepared.adapter = Arc::new(PanickingAdapter);
        execute_inference(prepared, guard).await;
        assert_eq!(
            rx.await.unwrap().unwrap_err().kind,
            InferenceErrorKind::TaskPanicked
        );
        assert_eq!(scope.activity_count(), 0);
        server.await.unwrap();
    }

    #[test]
    fn config_bounds_and_error_details_are_safe() {
        assert_eq!(parse_context_window(None).unwrap(), 1_000_000);
        assert_eq!(parse_context_window(Some("200K")).unwrap(), 200_000);
        for bad in ["0k", "1.5m", "18446744073709551615t", "1"] {
            assert!(parse_context_window(Some(bad)).is_err());
        }
        assert_eq!(
            provider_error_detail(br#"{"error":{"message":"quota\nexceeded"}}"#).as_deref(),
            Some("quota exceeded")
        );
        assert_eq!(
            safe_endpoint(&Url::parse("https://user:password@example.test/v1?key=secret").unwrap()),
            "https://example.test"
        );
        let error = InferenceError::new(InferenceErrorKind::RequestFailed, "你".repeat(300));
        assert!(error.message.len() <= 512);
    }

    #[tokio::test]
    async fn mock_http_half_stream_is_not_success() {
        let (url, server) = mock("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"stop\"}]}\n\n", false).await;
        let (prepared, guard, rx, scope) = prepared(&url, false);
        execute_inference(prepared, guard).await;
        assert_eq!(
            rx.await.unwrap().unwrap_err().kind,
            InferenceErrorKind::ResponseIncomplete
        );
        assert_eq!(scope.activity_count(), 0);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn turn_cancel_returns_normal_reply_after_http_cleanup() {
        let (url, server) = mock("", true).await;
        let (prepared, guard, rx, scope) = prepared(&url, false);
        let turn_cancel = guard.activity.turn_cancel.clone();
        let task = tokio::spawn(execute_inference(prepared, guard));
        tokio::task::yield_now().await;
        turn_cancel.cancel();
        assert_eq!(rx.await.unwrap().unwrap(), InferenceResponse::aborted());
        assert_eq!(scope.activity_count(), 0);
        assert!(!scope.is_closed());
        task.await.unwrap();
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn mock_http_cancel_releases_real_activity_before_reply() {
        let (url, server) = mock("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n", true).await;
        let (prepared, guard, rx, scope) = prepared(&url, false);
        let cancel = prepared.cancel.clone();
        let task = tokio::spawn(execute_inference(prepared, guard));
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();
        assert_eq!(
            rx.await.unwrap().unwrap_err().kind,
            InferenceErrorKind::Cancelled
        );
        assert_eq!(scope.activity_count(), 0);
        task.await.unwrap();
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn receiver_closed_stops_http_and_scope_cancel_is_merged() {
        for close_receiver in [true, false] {
            let (url, server) = mock("", true).await;
            let (prepared, guard, rx, scope) = prepared(&url, false);
            let task = tokio::spawn(execute_inference(prepared, guard));
            tokio::time::sleep(Duration::from_millis(30)).await;
            if close_receiver {
                drop(rx);
            } else {
                scope.request_close();
                assert_eq!(
                    rx.await.unwrap().unwrap_err().kind,
                    InferenceErrorKind::Cancelled
                );
            }
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(scope.activity_count(), 0);
            server.abort();
            let _ = server.await;
        }
    }

    #[tokio::test]
    async fn owner_abort_joins_http_worker_before_scope_completion() {
        let (url, server) = mock("", true).await;
        let (prepared, guard, rx, scope) = prepared(&url, false);
        let owner = tokio::spawn(execute_inference(prepared, guard));
        tokio::time::sleep(Duration::from_millis(30)).await;
        owner.abort();
        let _ = owner.await;
        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().kind, InferenceErrorKind::TaskAborted);
        scope.request_close();
        tokio::time::timeout(Duration::from_secs(1), scope.wait_all())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(scope.activity_count(), 0);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn owner_abort_during_terminal_does_not_poison_scope() {
        let (url, server) = mock("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n", false).await;
        let (mut prepared, guard, rx, scope) = prepared(&url, false);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        prepared.terminal_gate = Some((entered.clone(), release));
        let owner = tokio::spawn(execute_inference(prepared, guard));
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        owner.abort();
        let _ = owner.await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), rx)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .kind,
            InferenceErrorKind::TaskAborted
        );
        scope.request_close();
        tokio::time::timeout(Duration::from_secs(1), scope.wait_all())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(scope.activity_count(), 0);
        server.await.unwrap();
    }

    #[test]
    fn task_drop_is_cleanup_failure_not_fake_completion() {
        let scope = AgentExecutionScope::new();
        let activity = scope.register(ExecutionActivityKind::Inference).unwrap();
        activity.mark_started().unwrap();
        let (tx, mut rx) = oneshot::channel();
        drop(TaskGuard {
            activity,
            reply: Arc::new(Mutex::new(Some(tx))),
            started: true,
            finished: false,
        });
        assert_eq!(scope.activity_count(), 1);
        assert_eq!(
            rx.try_recv().unwrap().unwrap_err().kind,
            InferenceErrorKind::TaskAborted
        );
    }

    #[test]
    fn visible_snapshots_survive_reload_and_alias_conflicts_are_rejected() {
        let mut world = World::new();
        world.insert_resource(resource_id_plugin::ResourcePluginInstalled);
        world.insert_resource(resource_id_plugin::ResourceTypeRegistry::default());
        use resource_id_plugin::WorldResourceExt;
        world.register_library::<Agent>("agent").unwrap();
        let agent = world.spawn();
        let other = world.spawn();
        let id = ResourceID::parse("tool:test/echo").unwrap();
        let original = ToolResource {
            alias: "echo".into(),
            description: "Old description".into(),
            input_schema: serde_json::json!({"type":"object"}),
            script_path: PathBuf::new(),
        };
        let selected = CallableResource {
            resource_id: id.clone(),
            alias: "echo".into(),
            definition: Arc::new(CallableDefinitionSnapshot::Tool(original.clone())),
        };
        let mut set = ResourceSet::new();
        set.insert(id.clone(), original);
        world.insert_component(agent, set);
        use margatroid_types::agent_components::{
            AgentInfo, AgentState, ImportedValue, WorldAgentExt,
        };
        use margatroid_types::block_storage::AgentDatabaseHandle;
        let root = tempfile::tempdir().unwrap();
        let database = AgentDatabaseHandle::new(
            memory_plugin::SqliteAgentDatabase::open(root.path().join("agent.sqlite")).unwrap(),
        );
        let agent_id = ResourceID::parse("agent:test/main").unwrap();
        world.insert_component(
            agent,
            Agent {
                resourceid: agent_id.clone(),
                workspace: ResourceID::parse("workspace:test/main").unwrap(),
                sandbox: None,
                info: AgentInfo {
                    model: margatroid_types::resource_components::AgentImageModelConfig {
                        model: "mock".into(),
                        temperature: None,
                        max_output_tokens: None,
                        top_p: None,
                        stop: vec![],
                    },
                    project_root: PathBuf::new(),
                    home_root: PathBuf::new(),
                    image: ResourceID::parse("agent_image:test/main").unwrap(),
                    image_dependencies: Arc::from([id.clone()]),
                },
                lua: Default::default(),
                state: AgentState::Working,
                history: database.history(),
                database,
                import_bindings: Default::default(),
                mcl: Default::default(),
                execution: AgentExecutionScope::new(),
                turn: None,
            },
        );
        world
            .agent_bind_import(
                &agent_id,
                "echo".into(),
                ImportedValue::Tool(selected.clone()),
            )
            .unwrap();
        world
            .get_component_mut::<ResourceSet<ToolResource>>(agent)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .description = "New description".into();
        let (tx, mut rx) = oneshot::channel();
        let stale = InferenceRequest {
            id: String::new(),
            agent,
            messages: vec![],
            tools: vec![],
            turn: Some("old-turn".into()),
            cancel: CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(tx))),
        };
        handle_inference(&mut world, stale.clone());
        handle_inference(&mut world, stale.clone());
        assert_eq!(
            rx.try_recv().unwrap().unwrap(),
            InferenceResponse::aborted()
        );
        assert!(stale.reply.lock().unwrap().is_none());
        assert_eq!(
            world
                .get_component::<Agent>(agent)
                .unwrap()
                .execution
                .activity_count(),
            0
        );
        let definitions = collect_tool_definitions(&world, agent, &[selected.clone()]).unwrap();
        assert_eq!(definitions[0].description, "Old description");
        for kind in ["skill", "hook"] {
            let legacy = CallableResource {
                resource_id: ResourceID::parse(&format!("{kind}:test/legacy")).unwrap(),
                ..selected.clone()
            };
            assert_eq!(
                collect_tool_definitions(&world, agent, &[legacy])
                    .unwrap_err()
                    .kind,
                InferenceErrorKind::UnsupportedResource
            );
        }
        world
            .agent_bind_import(
                &agent_id,
                "renamed".into(),
                ImportedValue::Tool(CallableResource {
                    alias: "renamed".into(),
                    ..selected.clone()
                }),
            )
            .unwrap();
        assert_eq!(
            collect_tool_definitions(&world, agent, &[selected.clone()]).unwrap()[0].name,
            "echo"
        );
        assert!(collect_tool_definitions(&world, agent, &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            collect_tool_definitions(&world, other, &[selected.clone()])
                .unwrap_err()
                .kind,
            InferenceErrorKind::ResourceMissing
        );
        assert_eq!(
            collect_tool_definitions(&world, agent, &[selected.clone(), selected])
                .unwrap_err()
                .kind,
            InferenceErrorKind::InvalidToolDefinitions
        );
    }

    #[test]
    fn system_role_survives_normalization_and_provider_encoding() {
        let normalized = normalize_messages(vec![
            AgentMessage::System {
                content: "system prompt".into(),
            },
            AgentMessage::User {
                content: "user prompt".into(),
            },
        ])
        .unwrap();
        assert!(
            matches!(&normalized[0], ProviderMessage::System { content } if content == "system prompt")
        );
        let url = Url::parse("http://127.0.0.1:1/v1").unwrap();
        let adapter = OpenAiAdapterFactory
            .build(ProviderRouteInput {
                provider: None,
                base_url: &url,
                api_key: "mock",
                thinking: None,
                reasoning_effort: None,
            })
            .unwrap();
        let request = adapter
            .build_request(ProviderInput::new(
                "mock",
                &InferenceParameters::default(),
                &normalized,
                &[],
            ))
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        let message = AgentMessage::System {
            content: "system prompt".into(),
        };
        assert_eq!(
            serde_json::from_value::<AgentMessage>(serde_json::to_value(&message).unwrap())
                .unwrap(),
            message
        );
    }

    #[test]
    fn inject_expansion_checks_correlations_and_limits() {
        for message in [
            AgentMessage::aborted_assistant(),
            AgentMessage::Inject {
                messages: vec![AgentMessage::aborted_assistant()],
            },
        ] {
            assert_eq!(
                normalize_messages(vec![message]).unwrap_err().kind,
                InferenceErrorKind::InvalidMessages
            );
        }
        assert_eq!(
            normalize_messages(vec![AgentMessage::Inject {
                messages: vec![AgentMessage::User {
                    content: "ok".into()
                }]
            }])
            .unwrap()
            .len(),
            1
        );
        assert!(normalize_messages(vec![AgentMessage::ToolCall {
            resource_id: None,
            tool_call_id: "orphan".into(),
            tool_name: "echo".into(),
            content: "bad".into(),
            failed: true
        }])
        .is_err());
        let mut nested = AgentMessage::User {
            content: "hi".into(),
        };
        for _ in 0..34 {
            nested = AgentMessage::Inject {
                messages: vec![nested],
            };
        }
        assert!(normalize_messages(vec![nested]).is_err());
    }
}
