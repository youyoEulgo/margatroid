use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_image_loader_plugin::AgentImageModelConfig;
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{
    AppAsyncExt, AsyncContext, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt,
};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use futures_util::{FutureExt, StreamExt};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, AgentReference, Message, MessageIntent, ToolCall,
    ToolDefinition,
};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOOL_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_TOOL_COUNT: usize = 256;
const MAX_STOP_COUNT: usize = 64;
const MAX_STOP_BYTES: usize = 256;
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceErrorKind {
    InvalidModelId,
    ConfigPathUnavailable,
    ConfigReadFailed,
    ConfigDecodeFailed,
    DuplicateModelId,
    InvalidModelRoute,
    UnsupportedApiType,
    InvalidCommand,
    AgentNotAlive,
    InferenceSnapshotMissing,
    ModelRouteNotFound,
    InvalidParameters,
    InvalidMessages,
    InvalidToolDefinitions,
    UnsupportedInput,
    RequestBuildFailed,
    RequestFailed,
    ResponseStatus,
    ResponseDecodeFailed,
    ResponseIncomplete,
    TaskPanicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceError {
    kind: InferenceErrorKind,
    message: String,
    status: Option<u16>,
}

impl InferenceError {
    pub fn new(kind: InferenceErrorKind, message: impl Into<String>) -> Self {
        Self::with_status(kind, None, message)
    }

    pub fn with_status(
        kind: InferenceErrorKind,
        status: Option<u16>,
        message: impl Into<String>,
    ) -> Self {
        let mut message = message.into();
        const SUFFIX: &str = "...";
        const MAX_BYTES: usize = 512;
        if message.len() > MAX_BYTES {
            let mut boundary = MAX_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self {
            kind,
            message,
            status,
        }
    }

    pub fn kind(&self) -> InferenceErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn status(&self) -> Option<u16> {
        self.status
    }

    fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(
                formatter,
                "{:?} (HTTP {status}): {}",
                self.kind, self.message
            ),
            None => write!(formatter, "{:?}: {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for InferenceError {}

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
    model: ModelId,
    parameters: InferenceParameters,
    workspace: Entity,
    source_image: Entity,
}

impl AgentInferenceSnapshot {
    pub fn model(&self) -> &ModelId {
        &self.model
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
    model: String,
    adapter: ErasedProviderAdapter,
}

impl ConfiguredModelRoute {
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn adapter(&self) -> &ErasedProviderAdapter {
        &self.adapter
    }
}

#[derive(Clone)]
pub struct WorkspaceModelRoutes {
    routes: HashMap<ModelId, ConfiguredModelRoute>,
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

impl Component for WorkspaceModelRoutes {}

pub struct ProviderInput<'a> {
    model: &'a str,
    parameters: &'a InferenceParameters,
    messages: &'a [Message],
    tools: &'a [ToolDefinition],
}

impl<'a> ProviderInput<'a> {
    fn new(
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
    provider: Option<&'a str>,
    base_url: &'a Url,
    api_key: &'a str,
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
}

pub struct ProviderHttpRequest {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Vec<u8>,
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
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, InferenceError>;

    fn finish(self: Box<Self>) -> Result<InferenceResponse, InferenceError>;
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
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadModelRoutes {
    pub id: String,
}

impl Event for ReloadModelRoutes {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRoutesReloaded {
    pub route_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadModelRoutesResult {
    pub id: String,
    pub result: Result<ModelRoutesReloaded, InferenceError>,
}

impl Event for ReloadModelRoutesResult {}

#[derive(Clone, Debug)]
pub struct InferenceCommand {
    pub id: String,
    pub agent: Entity,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub stream: Option<InferenceStreamSender>,
}

impl Event for InferenceCommand {}

pub type InferenceStreamSender = tokio::sync::mpsc::Sender<String>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct InferenceRoute {
    id: String,
    agent: Entity,
}

struct PreparedInference {
    route: InferenceRoute,
    client: reqwest::Client,
    request: ProviderHttpRequest,
    adapter: ErasedProviderAdapter,
    stream: Option<InferenceStreamSender>,
}

impl Event for PreparedInference {}

struct InferenceTaskOutput {
    route: InferenceRoute,
    result: Result<InferenceResponse, InferenceError>,
}

struct InferenceTaskError {
    source: AsyncTaskError,
}

impl From<AsyncTaskError> for InferenceTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

pub struct GlobalModelRoutes {
    path: PathBuf,
    factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>,
    routes: HashMap<ModelId, ConfiguredModelRoute>,
}

impl GlobalModelRoutes {
    pub(crate) fn load(
        path: PathBuf,
        factories: Arc<HashMap<String, ErasedProviderAdapterFactory>>,
    ) -> Result<Self, InferenceError> {
        let routes = load_model_routes(&path, &factories)?;
        Ok(Self {
            path,
            factories,
            routes,
        })
    }

    pub(crate) fn reload(&mut self) -> Result<usize, InferenceError> {
        let routes = load_model_routes(&self.path, &self.factories)?;
        let count = routes.len();
        self.routes = routes;
        Ok(count)
    }

    pub(crate) fn get(&self, id: &ModelId) -> Option<ConfiguredModelRoute> {
        self.routes.get(id).cloned()
    }
}

impl Resource for GlobalModelRoutes {}

pub struct InferenceHttpClient {
    client: reqwest::Client,
}

impl InferenceHttpClient {
    pub(crate) fn new() -> Result<Self, InferenceError> {
        reqwest::Client::builder()
            .build()
            .map(|client| Self { client })
            .map_err(|_| {
                InferenceError::new(
                    InferenceErrorKind::RequestBuildFailed,
                    "inference HTTP client could not be created",
                )
            })
    }
}

impl Resource for InferenceHttpClient {}

pub struct InferencePlugin {
    schedule: String,
    config_path: PathBuf,
    adapter_factories: HashMap<String, ErasedProviderAdapterFactory>,
}

impl InferencePlugin {
    pub fn new() -> Self {
        let mut adapter_factories: HashMap<String, ErasedProviderAdapterFactory> = HashMap::new();
        adapter_factories.insert("openai".into(), Arc::new(OpenAiAdapterFactory));
        Self {
            schedule: RuntimePlugin::PRE_UPDATE.to_owned(),
            config_path: default_config_path(),
            adapter_factories,
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }

    pub fn with_config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = path.into();
        self
    }

    pub fn with_api_type<Factory>(mut self, api_type: impl Into<String>, factory: Factory) -> Self
    where
        Factory: ProviderAdapterFactory,
    {
        let api_type = api_type.into();
        if api_type.is_empty() || self.adapter_factories.contains_key(&api_type) {
            InferenceError::new(
                InferenceErrorKind::UnsupportedApiType,
                "api type is empty or already registered",
            )
            .panic();
        }
        self.adapter_factories.insert(api_type, Arc::new(factory));
        self
    }
}

impl Default for InferencePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for InferencePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "RuntimePlugin and AsyncRuntimePlugin are required",
            )
            .panic();
        }
        if app.world().contains_resource::<GlobalModelRoutes>()
            || app.world().contains_resource::<InferenceHttpClient>()
        {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "InferencePlugin is already installed",
            )
            .panic();
        }
        if !app.contains_schedule(&self.schedule) {
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "InferencePlugin schedule does not exist",
            )
            .panic();
        }
        let factories = Arc::new(self.adapter_factories);
        let routes = GlobalModelRoutes::load(self.config_path, Arc::clone(&factories))
            .unwrap_or_else(|error| error.panic());
        let client = InferenceHttpClient::new().unwrap_or_else(|error| error.panic());
        let schedule = self.schedule;
        app.world_mut().insert_resource(routes);
        app.world_mut().insert_resource(client);
        app.add_system(&schedule, reload_model_routes_system)
            .add_system(&schedule, prepare_inference_system)
            .add_async_system(&schedule, execute_prepared_inference)
            .add_system(&schedule, publish_inference_output_system);
    }
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
            self.remove_component::<WorkspaceModelRoutes>(workspace);
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
        self.insert_component(workspace, WorkspaceModelRoutes { routes });
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
        Ok(AgentInferenceSnapshot {
            model,
            parameters,
            workspace,
            source_image,
        })
    }
}

#[derive(Deserialize)]
struct ModelRouteDocument {
    models: Vec<ModelRouteConfig>,
}

#[derive(Deserialize)]
struct ModelRouteConfig {
    id: String,
    model: String,
    provider: Option<String>,
    base_url: String,
    api_key: String,
    api_type: String,
}

fn default_config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".margatroid")
        .join("models.toml")
}

fn load_model_routes(
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
        })?;
        routes.insert(
            id,
            ConfiguredModelRoute {
                model: config.model,
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
        .get_component::<WorkspaceModelRoutes>(workspace)
        .and_then(|routes| routes.get(model))
        .or_else(|| {
            world
                .get_resource::<GlobalModelRoutes>()
                .and_then(|routes| routes.get(model))
        })
        .is_some()
}

fn reload_model_routes_system(world: &mut World) {
    let requests = world
        .event_reader::<ReloadModelRoutes>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
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
                .map(|route_count| ModelRoutesReloaded { route_count })
        };
        world.send_event(ReloadModelRoutesResult {
            id: request.id,
            result,
        });
    }
}

fn prepare_inference_system(world: &mut World) {
    let commands = world
        .event_reader::<InferenceCommand>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for command in commands {
        match prepare_inference(world, command) {
            Ok(prepared) => world.send_async_event(prepared),
            Err((route, error)) => events.send_event(AgentFailure {
                id: route.id,
                agent: route.agent,
                kind: AgentFailureKind::Inference,
                message: error.to_string(),
            }),
        }
    }
}

fn prepare_inference(
    world: &World,
    command: InferenceCommand,
) -> Result<PreparedInference, (InferenceRoute, InferenceError)> {
    let route = InferenceRoute {
        id: command.id.clone(),
        agent: command.agent,
    };
    if command.id.is_empty() {
        return Err((
            route,
            InferenceError::new(
                InferenceErrorKind::InvalidCommand,
                "inference request ID cannot be empty",
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
    let snapshot = world
        .get_component::<AgentInferenceSnapshot>(command.agent)
        .ok_or_else(|| {
            (
                route.clone(),
                InferenceError::new(
                    InferenceErrorKind::InferenceSnapshotMissing,
                    "agent inference snapshot is missing",
                ),
            )
        })?;
    let model_route = world
        .get_component::<WorkspaceModelRoutes>(snapshot.workspace)
        .and_then(|routes| routes.get(&snapshot.model))
        .or_else(|| {
            world
                .get_resource::<GlobalModelRoutes>()
                .and_then(|routes| routes.get(&snapshot.model))
        })
        .ok_or_else(|| {
            (
                route.clone(),
                InferenceError::new(
                    InferenceErrorKind::ModelRouteNotFound,
                    format!("model route `{}` was not found", snapshot.model),
                ),
            )
        })?;
    let request = model_route
        .adapter
        .build_request(ProviderInput::new(
            &model_route.model,
            &snapshot.parameters,
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
    Ok(PreparedInference {
        route,
        client,
        request,
        adapter: model_route.adapter,
        stream: command.stream,
    })
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
            Message::Assistant {
                content,
                tool_calls,
            } => {
                content.as_deref().unwrap_or("").len()
                    + tool_calls
                        .iter()
                        .map(|call| call.id.len() + call.name.len() + call.arguments.len())
                        .sum::<usize>()
            }
            Message::Tool {
                tool_call_id,
                content,
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
        } = message
        {
            if content.is_none() && tool_calls.is_empty() {
                return Err(InferenceError::new(
                    InferenceErrorKind::InvalidMessages,
                    "assistant message must contain content or tool calls",
                ));
            }
            for call in tool_calls {
                if call.id.is_empty() || call.name.is_empty() {
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

async fn execute_prepared_inference(
    prepared: PreparedInference,
    _context: AsyncContext,
) -> Result<InferenceTaskOutput, InferenceTaskError> {
    let route = prepared.route.clone();
    let result = std::panic::AssertUnwindSafe(run_provider(prepared))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(InferenceError::new(
                InferenceErrorKind::TaskPanicked,
                "inference provider task panicked",
            ))
        });
    Ok(InferenceTaskOutput { route, result })
}

async fn run_provider(prepared: PreparedInference) -> Result<InferenceResponse, InferenceError> {
    let request = reqwest::Request::new(prepared.request.method, prepared.request.url);
    let mut request = request;
    *request.headers_mut() = prepared.request.headers;
    *request.body_mut() = Some(reqwest::Body::from(prepared.request.body));
    let response = prepared.client.execute(request).await.map_err(|_| {
        InferenceError::new(
            InferenceErrorKind::RequestFailed,
            "inference provider request failed",
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        let _ = read_bounded_body(response, MAX_ERROR_BODY_BYTES).await;
        return Err(InferenceError::with_status(
            InferenceErrorKind::ResponseStatus,
            Some(status.as_u16()),
            "inference provider returned a non-success status",
        ));
    }
    let headers = response.headers().clone();
    let mut accumulator = prepared.adapter.begin_response(status, &headers)?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| {
            InferenceError::new(
                InferenceErrorKind::RequestFailed,
                "inference response stream failed",
            )
        })?;
        let text = accumulator.push(&chunk)?;
        if let Some(sender) = &prepared.stream {
            for text in text {
                if sender.send(text).await.is_err() {
                    break;
                }
            }
        }
    }
    accumulator.finish()
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

fn publish_inference_output_system(world: &mut World) {
    let outputs = world
        .event_reader::<Result<InferenceTaskOutput, InferenceTaskError>>()
        .into_iter()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for output in outputs {
        match output {
            Ok(output) => match &output.result {
                Ok(response) => {
                    let intent = match &response.message {
                        Message::Assistant { tool_calls, .. } if tool_calls.is_empty() => {
                            MessageIntent::CompleteTurn
                        }
                        Message::Assistant { tool_calls, .. } => {
                            if tool_calls
                                .iter()
                                .any(|call| call.id.is_empty() || call.name.is_empty())
                            {
                                events.send_event(AgentFailure {
                                    id: output.route.id.clone(),
                                    agent: output.route.agent,
                                    kind: AgentFailureKind::Inference,
                                    message: InferenceError::new(
                                        InferenceErrorKind::ResponseIncomplete,
                                        "inference response contains an invalid tool call",
                                    )
                                    .to_string(),
                                });
                                continue;
                            }
                            MessageIntent::DispatchToolCalls
                        }
                        _ => {
                            events.send_event(AgentFailure {
                                id: output.route.id.clone(),
                                agent: output.route.agent,
                                kind: AgentFailureKind::Inference,
                                message: InferenceError::new(
                                    InferenceErrorKind::UnsupportedInput,
                                    "inference provider returned a non-assistant message",
                                )
                                .to_string(),
                            });
                            continue;
                        }
                    };
                    events.send_event(AgentMessage {
                        id: output.route.id.clone(),
                        agent: AgentReference::Entity(output.route.agent),
                        message: response.message.clone(),
                        intent,
                    });
                }
                Err(error) => events.send_event(AgentFailure {
                    id: output.route.id.clone(),
                    agent: output.route.agent,
                    kind: AgentFailureKind::Inference,
                    message: error.to_string(),
                }),
            },
            Err(error) => tracing::warn!(error = %error.source, "inference task was cancelled"),
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
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
            temperature: input.parameters().temperature(),
            max_tokens: input.parameters().max_output_tokens(),
            top_p: input.parameters().top_p(),
            stop: input.parameters().stop().to_vec(),
        }
    }
}

fn openai_message(message: &Message) -> serde_json::Value {
    match message {
        Message::System { content } => serde_json::json!({"role":"system", "content":content}),
        Message::User { content } => serde_json::json!({"role":"user", "content":content}),
        Message::Assistant {
            content,
            tool_calls,
        } => serde_json::json!({
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls.iter().map(|call| serde_json::json!({
                "id": call.id,
                "type": "function",
                "function": {"name": call.name, "arguments": call.arguments}
            })).collect::<Vec<_>>(),
        }),
        Message::Tool {
            tool_call_id,
            content,
        } => serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

#[derive(Default)]
struct OpenAiAccumulator {
    buffer: Vec<u8>,
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
    total_tokens: u64,
}

impl ProviderResponseAccumulator for OpenAiAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, InferenceError> {
        self.buffer.extend_from_slice(chunk);
        let mut text = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=index).collect::<Vec<_>>();
            text.extend(self.consume_line(&line[..line.len() - 1])?);
        }
        Ok(text)
    }

    fn finish(mut self: Box<Self>) -> Result<InferenceResponse, InferenceError> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.consume_line(&line)?;
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
                name: call.name,
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
        Ok(InferenceResponse {
            message: Message::Assistant {
                content: (!self.content.is_empty()).then_some(self.content),
                tool_calls: calls,
            },
            stop_reason: reason,
            usage: self.usage,
        })
    }
}

impl OpenAiAccumulator {
    fn consume_line(&mut self, line: &[u8]) -> Result<Vec<String>, InferenceError> {
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

    fn consume_chunk(&mut self, chunk: OpenAiChunk) -> Result<Vec<String>, InferenceError> {
        if let Some(usage) = chunk.usage {
            self.usage = Some(TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
            });
        }
        let mut output = Vec::new();
        for choice in chunk.choices {
            self.saw_choice = true;
            let delta = choice.delta.or(choice.message).unwrap_or_default();
            if let Some(content) = delta.content {
                if self.content.len().saturating_add(content.len()) > MAX_MESSAGE_BYTES {
                    return Err(InferenceError::new(
                        InferenceErrorKind::ResponseDecodeFailed,
                        "inference response content exceeds the size limit",
                    ));
                }
                self.content.push_str(&content);
                output.push(content);
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
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn model_routes_compile_and_openai_accumulator_handles_split_sse() {
        let mut factories = HashMap::new();
        factories.insert(
            "openai".to_owned(),
            Arc::new(OpenAiAdapterFactory) as ErasedProviderAdapterFactory,
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

        let mut accumulator = OpenAiAccumulator::default();
        let first = br#"data: {"choices":[{"delta":{"role":"assistant","content":"he"},"finish_reason":null}]}

"#;
        let second = br#"data: {"choices":[{"delta":{"content":"llo"},"finish_reason":"stop"}]}

data: [DONE]

"#;
        let mut visible = accumulator.push(&first[..13]).unwrap();
        visible.extend(accumulator.push(&first[13..]).unwrap());
        visible.extend(accumulator.push(second).unwrap());
        let response = Box::new(accumulator).finish().unwrap();
        assert_eq!(visible, ["he", "llo"]);
        assert_eq!(
            response.message,
            Message::Assistant {
                content: Some("hello".into()),
                tool_calls: Vec::new(),
            }
        );
    }

    #[test]
    fn openai_accumulator_reassembles_tool_call_fragments() {
        let mut accumulator = OpenAiAccumulator::default();
        accumulator
            .push(
                br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"echo","arguments":"{\"te"}}]},"finish_reason":null}]}

"#,
            )
            .unwrap();
        accumulator
            .push(
                br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"xt\":\"hi\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
            )
            .unwrap();
        let response = Box::new(accumulator).finish().unwrap();
        assert_eq!(
            response.message,
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"hi"}"#.into(),
                }],
            }
        );
        assert_eq!(response.stop_reason, StopReason::ToolCalls);
    }

    #[test]
    fn inference_plugin_loads_routes_and_rejects_missing_snapshots() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("models.toml");
        fs::write(
            &path,
            r#"[[models]]
id = "test"
model = "test-model"
base_url = "https://example.test/v1"
api_key = "secret"
api_type = "openai"
"#,
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(InferencePlugin::default().with_config_path(path));
        let agent = app.world_mut().spawn();
        app.world().send_event(InferenceCommand {
            id: "request".into(),
            agent,
            messages: vec![Message::User {
                content: "hello".into(),
            }],
            tools: Vec::new(),
            stream: None,
        });
        app.tick();
        app.tick();
        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.id, "request");
        assert_eq!(failure.kind, AgentFailureKind::Inference);
        assert_eq!(
            failure.message,
            "InferenceSnapshotMissing: agent inference snapshot is missing"
        );
    }
}
