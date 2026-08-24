use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::fs;
use std::path::{Path, PathBuf};

use agent_image_loader_plugin::{AgentImage, AgentImageModelConfig};
use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use config_plugin::{MargatroidConfig, WebSocketMessageTarget};
use core_plugin::{Entity, Resource, World};
use futures_util::StreamExt;
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, CapturedInferenceResponse, Message, ResourceId,
    ToolDefinition,
};
use reqwest::Url;
use serde::Serialize;
use server_plugin::{
    WebSocketConnections, WebSocketMessage, WebSocketMessageSender, WebSocketSender,
};
use tokio::sync::watch;

use crate::events::{
    CancelInferenceRequest, ContextCompactionInferenceResponse, InferenceOutputKind,
    InferenceRoute, InferenceTaskOutput, InferenceTaskResult, PreparedInference, ReloadModelRoutes,
};
use crate::types::{
    AgentInferenceSnapshot, ConfiguredModelRoute, ErasedProviderAdapterFactory,
    InferenceParameters, ModelId, ModelRouteDocument, ProviderHttpRequest,
    ProviderInferenceResponse, ProviderInput, ProviderRouteInput, ProviderStreamDelta, StopReason,
    WorkspaceModelRoutes, WorkspaceModelRoutesRegistry, MAX_CONFIG_BYTES, MAX_ERROR_BODY_BYTES,
    MAX_MESSAGES_BYTES, MAX_MESSAGE_BYTES, MAX_TOOL_COUNT, MAX_TOOL_DESCRIPTION_BYTES,
};
use crate::{GlobalModelRoutes, InferenceError, InferenceErrorKind, InferenceHttpClient};

#[derive(Default)]
pub(crate) struct InFlightInferences {
    pub(crate) requests: HashMap<(Entity, String), watch::Sender<bool>>,
}

impl Resource for InFlightInferences {}

pub(crate) struct InferenceCommand {
    pub(crate) id: String,
    pub(crate) agent: Entity,
    pub(crate) agent_id: ResourceId,
    pub(crate) messages: Vec<Message>,
    pub(crate) tools: Vec<ToolDefinition>,
    pub(crate) output: InferenceOutputKind,
}

pub trait WorldInferenceExt {
    fn reload_model_routes(&self, id: impl Into<String>);

    fn load_workspace_model_routes(
        &mut self,
        workspace: Entity,
        project_root: &Path,
    ) -> Result<usize, InferenceError>;

    fn build_agent_inference_snapshot(
        &self,
        workspace: Entity,
        source_image: Entity,
        config: &AgentImageModelConfig,
    ) -> Result<AgentInferenceSnapshot, InferenceError>;
}

impl WorldInferenceExt for World {
    fn reload_model_routes(&self, id: impl Into<String>) {
        self.send_event(ReloadModelRoutes { id: id.into() });
    }

    fn load_workspace_model_routes(
        &mut self,
        workspace: Entity,
        project_root: &Path,
    ) -> Result<usize, InferenceError> {
        require_workspace(self, workspace)?;
        let path = project_root.join(".margatroid").join("models.toml");
        if !path.exists() {
            self.get_resource_mut::<WorkspaceModelRoutesRegistry>()
                .ok_or_else(|| {
                    InferenceError::new(
                        InferenceErrorKind::InvalidCommand,
                        "workspace model route registry is missing",
                    )
                })?
                .remove(workspace);
            return Ok(0);
        }
        let factories = self
            .get_resource::<GlobalModelRoutes>()
            .ok_or_else(|| {
                InferenceError::new(
                    InferenceErrorKind::InvalidCommand,
                    "InferencePlugin is not installed",
                )
            })?
            .factories
            .clone();
        let routes = load_model_routes(&path, &factories)?;
        let count = routes.len();
        self.get_resource_mut::<WorkspaceModelRoutesRegistry>()
            .ok_or_else(|| {
                InferenceError::new(
                    InferenceErrorKind::InvalidCommand,
                    "workspace model route registry is missing",
                )
            })?
            .insert(workspace, WorkspaceModelRoutes { routes });
        Ok(count)
    }

    fn build_agent_inference_snapshot(
        &self,
        workspace: Entity,
        source_image: Entity,
        config: &AgentImageModelConfig,
    ) -> Result<AgentInferenceSnapshot, InferenceError> {
        require_workspace(self, workspace)?;
        if !self.is_alive(source_image) {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "source AgentImage entity is not alive",
            ));
        }
        let model = ModelId::new(config.model())?;
        let source = config.parameters();
        let parameters = InferenceParameters::new(
            source.temperature(),
            source.max_output_tokens(),
            source.top_p(),
            source.stop().to_vec(),
        );
        parameters.validate()?;
        if !model_is_routable(self, workspace, &model) {
            return Err(InferenceError::new(
                InferenceErrorKind::ModelRouteNotFound,
                format!("model route `{model}` was not found"),
            ));
        }
        let context_window_tokens = self
            .get_resource::<WorkspaceModelRoutesRegistry>()
            .and_then(|registry| registry.get(workspace))
            .and_then(|routes| routes.get(&model))
            .or_else(|| {
                self.get_resource::<GlobalModelRoutes>()
                    .and_then(|routes| routes.get(&model))
            })
            .map(|route| route.context_window_tokens())
            .unwrap_or(1_000_000);
        Ok(AgentInferenceSnapshot {
            model,
            context_window_tokens,
            parameters,
            workspace,
            source_image,
        })
    }
}
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
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            InferenceError::new(
                InferenceErrorKind::InvalidModelRoute,
                "context_window must be greater than zero",
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
    let source = fs::read_to_string(path).map_err(|_| {
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
    compile_model_routes(&source, factories)
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
        let adapter = factory.build(ProviderRouteInput {
            provider: config.provider.as_deref(),
            base_url: &base_url,
            api_key: &config.api_key,
            thinking: config.thinking.as_deref(),
            reasoning_effort: config.reasoning_effort.as_deref(),
        })?;
        let context_window_tokens = parse_context_window(config.context_window.as_deref())?;
        routes.insert(
            id,
            ConfiguredModelRoute {
                model: config.model,
                context_window_tokens,
                adapter,
            },
        );
    }
    Ok(routes)
}

fn require_workspace(world: &World, workspace: Entity) -> Result<(), InferenceError> {
    if world.is_alive(workspace) {
        Ok(())
    } else {
        Err(InferenceError::new(
            InferenceErrorKind::AgentNotAlive,
            "workspace entity is not alive",
        ))
    }
}

fn model_is_routable(world: &World, workspace: Entity, model: &ModelId) -> bool {
    world
        .get_resource::<WorkspaceModelRoutesRegistry>()
        .and_then(|registry| registry.get(workspace))
        .and_then(|routes| routes.get(model))
        .or_else(|| {
            world
                .get_resource::<GlobalModelRoutes>()
                .and_then(|routes| routes.get(model))
        })
        .is_some()
}

pub(crate) fn handle_reload_model_routes(
    world: &mut World,
    request: crate::events::ReloadModelRoutes,
) {
    let result = if request.id.is_empty() {
        Err(InferenceError::new(
            InferenceErrorKind::InvalidCommand,
            "route reload ID cannot be empty",
        ))
    } else {
        world
            .get_resource_mut::<GlobalModelRoutes>()
            .ok_or_else(|| {
                InferenceError::new(
                    InferenceErrorKind::InvalidCommand,
                    "InferencePlugin is not installed",
                )
            })
            .and_then(GlobalModelRoutes::reload)
            .map(|route_count| crate::types::ModelRoutesReloaded { route_count })
    };
    world.send_event(crate::events::ReloadModelRoutesResult {
        id: request.id,
        result,
    });
}

pub(crate) fn handle_prepare_inference(world: &mut World, command: InferenceCommand) {
    match prepare_inference(world, command) {
        Ok(prepared) => world.send_async_event(prepared),
        Err((route, error)) => {
            let events = world.event_sender();
            publish_inference_error(&events, route, error);
        }
    }
}

fn prepare_inference(
    world: &mut World,
    command: InferenceCommand,
) -> Result<PreparedInference, (InferenceRoute, InferenceError)> {
    let route = InferenceRoute {
        id: command.id.clone(),
        agent: command.agent,
        output: command.output,
    };
    if command.id.is_empty() || command.agent_id.resource_type() != "agent" {
        return Err((
            route,
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "inference request and Agent IDs must be valid",
            ),
        ));
    }
    if !world.is_alive(command.agent) {
        return Err((
            route,
            InferenceError::new(
                InferenceErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ),
        ));
    }
    validate_messages(&command.messages).map_err(|error| (route.clone(), error))?;
    validate_tools(&command.tools).map_err(|error| (route.clone(), error))?;
    let agent = world.get_component::<Agent>(command.agent).ok_or_else(|| {
        (
            route.clone(),
            InferenceError::new(
                InferenceErrorKind::InferenceSnapshotMissing,
                "agent inference snapshot is missing",
            ),
        )
    })?;
    let image = world
        .get_component::<AgentImage>(agent.info.image_entity)
        .ok_or_else(|| {
            (
                route.clone(),
                InferenceError::new(
                    InferenceErrorKind::InferenceSnapshotMissing,
                    "agent source image is missing",
                ),
            )
        })?;
    let model = ModelId::new(agent.info.model.model.clone()).map_err(|error| {
        (
            route.clone(),
            InferenceError::new(InferenceErrorKind::InvalidModelId, error.to_string()),
        )
    })?;
    let parameters = InferenceParameters::new(
        image.model().parameters().temperature(),
        image.model().parameters().max_output_tokens(),
        image.model().parameters().top_p(),
        image.model().parameters().stop().to_vec(),
    );
    let workspace = agent.info.workspace_id;
    let model_route = world
        .get_resource::<WorkspaceModelRoutesRegistry>()
        .and_then(|registry| registry.get(workspace))
        .and_then(|routes| routes.get(&model))
        .or_else(|| {
            world
                .get_resource::<GlobalModelRoutes>()
                .and_then(|routes| routes.get(&model))
        })
        .ok_or_else(|| {
            (
                route.clone(),
                InferenceError::new(
                    InferenceErrorKind::ModelRouteNotFound,
                    format!("model route `{}` was not found", model),
                ),
            )
        })?;
    let adapter = model_route.adapter().clone();
    let request = adapter
        .build_request(ProviderInput::new(
            model_route.model(),
            &parameters,
            &command.messages,
            &command.tools,
        ))
        .map_err(|error| (route.clone(), error))?;
    let client = world
        .get_resource::<InferenceHttpClient>()
        .ok_or_else(|| {
            (
                route.clone(),
                InferenceError::new(
                    InferenceErrorKind::InvalidCommand,
                    "inference HTTP client is missing",
                ),
            )
        })?
        .client
        .clone();
    let senders = match route.output {
        InferenceOutputKind::AgentMessage => {
            let targets = world
                .get_resource::<MargatroidConfig>()
                .ok_or_else(|| {
                    (
                        route.clone(),
                        InferenceError::new(
                            InferenceErrorKind::InvalidCommand,
                            "global configuration is missing",
                        ),
                    )
                })?
                .streaming_member_messages();
            let connections = world
                .get_resource::<WebSocketConnections>()
                .ok_or_else(|| {
                    (
                        route.clone(),
                        InferenceError::new(
                            InferenceErrorKind::InvalidCommand,
                            "WebSocket connection registry is missing",
                        ),
                    )
                })?;
            resolve_websocket_targets(connections, targets)
        }
        InferenceOutputKind::ContextCompaction | InferenceOutputKind::Captured => Vec::new(),
    };
    let (cancellation_sender, cancellation) = watch::channel(false);
    world
        .get_resource_mut::<InFlightInferences>()
        .expect("InferencePlugin is installed")
        .requests
        .insert((route.agent, route.id.clone()), cancellation_sender);
    Ok(PreparedInference {
        route,
        agent_id: command.agent_id,
        client,
        request,
        adapter,
        senders,
        cancellation,
    })
}

pub(crate) fn handle_cancel_inference(world: &mut World, cancellation: CancelInferenceRequest) {
    let in_flight = world
        .get_resource_mut::<InFlightInferences>()
        .expect("InferencePlugin is installed");
    if let Some(sender) = in_flight
        .requests
        .get(&(cancellation.agent, cancellation.id))
    {
        sender.send_replace(true);
    }
}

fn resolve_websocket_targets(
    connections: &WebSocketConnections,
    targets: &[WebSocketMessageTarget],
) -> Vec<WebSocketSender> {
    let mut seen = HashSet::new();
    targets
        .iter()
        .flat_map(|target| match target {
            WebSocketMessageTarget::Broadcast => connections.get_all(),
            WebSocketMessageTarget::Type(connection_type) => {
                connections.get_by_type(connection_type)
            }
            WebSocketMessageTarget::Name(name) => {
                connections.get_by_name(name).into_iter().collect()
            }
        })
        .filter(|sender| seen.insert(sender.connection_id()))
        .collect()
}
fn validate_messages(messages: &[Message]) -> Result<(), InferenceError> {
    if messages.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidMessages,
            "inference messages cannot be empty",
        ));
    }
    let mut total_bytes = 0usize;
    for message in messages {
        let size = match message {
            Message::System { content } | Message::User { content } => content.len(),
            Message::Error { message } => message.len(),
            Message::Assistant {
                reasoning,
                content,
                tool_calls,
            } => {
                reasoning.as_deref().unwrap_or("").len()
                    + content.as_deref().unwrap_or("").len()
                    + tool_calls
                        .iter()
                        .map(|call| call.id.len() + call.tool_name.len() + call.arguments.len())
                        .sum::<usize>()
            }
            Message::Tool {
                tool_call_id,
                content,
                ..
            } => {
                if tool_call_id.is_empty() {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "tool message call ID cannot be empty",
                    ));
                }
                tool_call_id.len() + content.len()
            }
        };
        if size > MAX_MESSAGE_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "one inference message exceeds the size limit",
            ));
        }
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > MAX_MESSAGES_BYTES {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidMessages,
                "inference messages exceed the total size limit",
            ));
        }
        if let Message::Assistant {
            content,
            tool_calls,
            ..
        } = message
        {
            if content.is_none() && tool_calls.is_empty() {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "assistant message must contain content or tool calls",
                ));
            }
            for call in tool_calls {
                if call.id.is_empty() {
                    return Err(InferenceError::new(
                        InferenceErrorKind::InvalidMessages,
                        "assistant tool call ID and name cannot be empty",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn validate_tools(tools: &[ToolDefinition]) -> Result<(), InferenceError> {
    if tools.len() > MAX_TOOL_COUNT {
        return Err(InferenceError::new(
            InferenceErrorKind::InvalidToolDefinitions,
            "inference tool count exceeds the limit",
        ));
    }
    let mut names = HashSet::new();
    for tool in tools {
        if tool.name.is_empty()
            || tool.name.len() > 64
            || !tool
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || tool.description.trim().is_empty()
            || tool.description.len() > MAX_TOOL_DESCRIPTION_BYTES
            || !tool.input_schema.is_object()
            || !names.insert(tool.name.clone())
        {
            return Err(InferenceError::new(
                InferenceErrorKind::InvalidToolDefinitions,
                "inference tool definitions are invalid or duplicated",
            ));
        }
    }
    Ok(())
}
pub(crate) async fn run_provider(
    prepared: PreparedInference,
) -> Result<ProviderInferenceResponse, InferenceError> {
    let endpoint = safe_endpoint(&prepared.request.url);
    let response = send_provider_request(&prepared.client, prepared.request).await?;
    let status = response.status();
    if !status.is_success() {
        let body = read_bounded_body(response, MAX_ERROR_BODY_BYTES).await;
        let detail = provider_error_detail(&body)
            .map(|detail| format!("inference provider rejected the request: {detail}"))
            .unwrap_or_else(|| "inference provider returned an empty error response".into());
        return Err(InferenceError::with_status(
            InferenceErrorKind::ResponseStatus,
            Some(status.as_u16()),
            detail,
        ));
    }
    let headers = response.headers().clone();
    let mut accumulator = prepared.adapter.begin_response(status, &headers)?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| {
            InferenceError::new(
                InferenceErrorKind::RequestFailed,
                format!(
                    "inference response stream failed at {endpoint}: {}",
                    summarize_reqwest_error(&error)
                ),
            )
        })?;
        for delta in accumulator.push(&chunk)? {
            send_stream_delta(
                &prepared.senders,
                &prepared.route.id,
                &prepared.agent_id,
                delta,
            )
            .await?;
        }
    }
    let (response, deltas) = accumulator.finish()?;
    for delta in deltas {
        send_stream_delta(
            &prepared.senders,
            &prepared.route.id,
            &prepared.agent_id,
            delta,
        )
        .await?;
    }
    Ok(response)
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

#[derive(Serialize)]
struct AgentMessageDeltaFrame<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    id: &'a str,
    agent: &'a ResourceId,
    content: &'a str,
}
async fn send_stream_delta(
    senders: &[WebSocketSender],
    id: &str,
    agent: &ResourceId,
    delta: ProviderStreamDelta,
) -> Result<(), InferenceError> {
    let (message_type, content) = match &delta {
        ProviderStreamDelta::Reasoning(content) => {
            ("agent.message.reasoning_delta", content.as_str())
        }
        ProviderStreamDelta::Content(content) => ("agent.message.delta", content.as_str()),
    };
    let encoded = serde_json::to_string(&AgentMessageDeltaFrame {
        message_type,
        id,
        agent,
        content,
    })
    .map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::ResponseEncodeFailed,
            "inference stream message could not be encoded",
        )
    })?;
    let active = senders
        .iter()
        .filter(|sender| !sender.is_closed())
        .cloned()
        .collect::<Vec<_>>();
    WebSocketMessageSender::new(active, WebSocketMessage::Text(encoded.into()))
        .send()
        .await;
    Ok(())
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
    let mut source = StdError::source(error);
    let mut deepest = None;
    while let Some(current) = source {
        deepest = Some(current.to_string());
        source = current.source();
    }
    match deepest.filter(|detail| !detail.trim().is_empty()) {
        Some(detail) => format!("{category}: {}", single_line(&detail)),
        None => category.into(),
    }
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

pub(crate) fn handle_inference_task_output(world: &mut World, output: InferenceTaskOutput) {
    let cancelled = world
        .get_resource_mut::<InFlightInferences>()
        .expect("InferencePlugin is installed")
        .requests
        .remove(&(output.route.agent, output.route.id.clone()))
        .is_some_and(|sender| *sender.borrow());
    if cancelled || matches!(output.result, InferenceTaskResult::Cancelled) {
        return;
    }
    let InferenceTaskResult::Completed(result) = &output.result else {
        unreachable!();
    };
    let events = world.event_sender();
    match result {
        Ok(response) => match output.route.output {
            InferenceOutputKind::AgentMessage => {
                if response.content.is_none() && response.tool_calls.is_empty() {
                    publish_inference_error(
                        &events,
                        output.route.clone(),
                        InferenceError::new(
                            InferenceErrorKind::ResponseIncomplete,
                            "inference response contains an invalid tool call",
                        ),
                    );
                    return;
                }
                events.send_event(AgentMessage {
                    id: output.route.id.clone(),
                    agent: output.route.agent,
                    message: Message::Assistant {
                        reasoning: response.reasoning.clone(),
                        content: response.content.clone(),
                        tool_calls: response.tool_calls.clone(),
                    },
                    usage: response.usage.clone(),
                });
            }
            InferenceOutputKind::ContextCompaction => {
                let result = context_compaction_content(response);
                events.send_event(ContextCompactionInferenceResponse {
                    id: output.route.id.clone(),
                    agent: output.route.agent,
                    result,
                });
            }
            InferenceOutputKind::Captured => {
                let result = context_compaction_content(response);
                events.send_event(CapturedInferenceResponse {
                    id: output.route.id.clone(),
                    agent: output.route.agent,
                    result: result.map_err(|error| error.to_string()),
                });
            }
        },
        Err(error) => publish_inference_error(&events, output.route.clone(), error.clone()),
    }
}

fn context_compaction_content(
    response: &ProviderInferenceResponse,
) -> Result<String, InferenceError> {
    if response.stop_reason != StopReason::Completed {
        return Err(InferenceError::new(
            InferenceErrorKind::ResponseIncomplete,
            "context compaction inference did not complete normally",
        ));
    }
    if !response.tool_calls.is_empty() {
        return Err(InferenceError::new(
            InferenceErrorKind::ResponseIncomplete,
            "context compaction inference returned tool calls",
        ));
    }
    let content = response
        .content
        .as_deref()
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .ok_or_else(|| {
            InferenceError::new(
                InferenceErrorKind::ResponseIncomplete,
                "context compaction inference returned no summary content",
            )
        })?;
    Ok(content.to_owned())
}
pub(crate) fn publish_inference_error(
    events: &app_runtime_plugin::RuntimeEventSender,
    route: InferenceRoute,
    error: InferenceError,
) {
    match route.output {
        InferenceOutputKind::AgentMessage => events.send_event(AgentFailure {
            id: route.id,
            agent: route.agent,
            kind: AgentFailureKind::Inference,
            message: error.to_string(),
        }),
        InferenceOutputKind::ContextCompaction => {
            events.send_event(ContextCompactionInferenceResponse {
                id: route.id,
                agent: route.agent,
                result: Err(error),
            })
        }
        InferenceOutputKind::Captured => {
            events.send_event(margatroid_types::CapturedInferenceResponse {
                id: route.id,
                agent: route.agent,
                result: Err(error.to_string()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::CancelInferenceRequest;
    use crate::system::cancel_inference_system;
    use app_runtime_plugin::RuntimePlugin;
    use core_plugin::App;
    use margatroid_types::ToolCall;
    use reqwest::Url;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::watch;

    #[test]
    fn context_window_uses_decimal_units_and_defaults_to_one_million() {
        assert_eq!(parse_context_window(None).unwrap(), 1_000_000);
        assert_eq!(parse_context_window(Some("200k")).unwrap(), 200_000);
        assert_eq!(parse_context_window(Some("500K")).unwrap(), 500_000);
        assert_eq!(parse_context_window(Some("1m")).unwrap(), 1_000_000);
        assert_eq!(parse_context_window(Some("3b")).unwrap(), 3_000_000_000);
        assert_eq!(parse_context_window(Some("2t")).unwrap(), 2_000_000_000_000);
        assert!(parse_context_window(Some("1g")).is_err());
        assert!(parse_context_window(Some("200000")).is_err());
        assert!(parse_context_window(Some("1.5m")).is_err());
    }

    #[test]
    fn provider_error_detail_extracts_json_and_sanitizes_text() {
        assert_eq!(
            provider_error_detail(br#"{"error":{"message":"quota\nexceeded"}}"#).as_deref(),
            Some("quota exceeded")
        );
        assert_eq!(
            provider_error_detail(b"upstream\n\x1b[31m unavailable").as_deref(),
            Some("upstream [31m unavailable")
        );
        assert_eq!(provider_error_detail(b""), None);
    }

    #[test]
    fn safe_endpoint_excludes_credentials_and_query_parameters() {
        let url =
            Url::parse("https://user:password@example.test:8443/v1/chat/completions?token=secret")
                .unwrap();
        assert_eq!(safe_endpoint(&url), "https://example.test:8443");
    }

    #[test]
    fn context_compaction_accepts_only_complete_text_without_tool_calls() {
        let complete = ProviderInferenceResponse {
            reasoning: Some("private reasoning".into()),
            content: Some("  summary  ".into()),
            tool_calls: Vec::new(),
            stop_reason: StopReason::Completed,
            usage: None,
        };
        assert_eq!(context_compaction_content(&complete).unwrap(), "summary");

        let truncated = ProviderInferenceResponse {
            stop_reason: StopReason::Length,
            ..complete.clone()
        };
        assert_eq!(
            context_compaction_content(&truncated).unwrap_err().kind(),
            InferenceErrorKind::ResponseIncomplete
        );

        let tool_call = ProviderInferenceResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                tool_name: "tool0".into(),
                arguments: "{}".into(),
            }],
            stop_reason: StopReason::ToolCalls,
            ..complete
        };
        assert_eq!(
            context_compaction_content(&tool_call).unwrap_err().kind(),
            InferenceErrorKind::ResponseIncomplete
        );
    }

    #[test]
    fn cancellation_marks_the_matching_inference() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default());
        app.world_mut()
            .insert_resource(InFlightInferences::default());
        app.add_system(RuntimePlugin::UPDATE, cancel_inference_system);
        let agent = app.world_mut().spawn();
        let (sender, receiver) = watch::channel(false);
        app.world_mut()
            .get_resource_mut::<InFlightInferences>()
            .unwrap()
            .requests
            .insert((agent, "turn-1".into()), sender);

        app.world().send_event(CancelInferenceRequest {
            id: "turn-1".into(),
            agent,
        });
        app.tick();

        assert!(*receiver.borrow());
    }

    #[test]
    fn model_routes_compile_and_reject_missing_factories() {
        let mut factories = HashMap::new();
        factories.insert(
            "openai".to_owned(),
            Arc::new(crate::OpenAiAdapterFactory) as crate::ErasedProviderAdapterFactory,
        );
        let routes = compile_model_routes(
            r#"[[models]]
id = "test"
model = "test-model"
base_url = "https://example.test/v1"
api_key = "secret"
api_type = "openai"
"#,
            &factories,
        )
        .unwrap();
        let route = routes.get(&ModelId::new("test").unwrap()).unwrap();
        assert_eq!(route.model(), "test-model");
        assert!(compile_model_routes("", &factories).is_err());
    }
}
