use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_image_loader_plugin::{AgentImage, AgentImageModelConfig};
use agent_plugin::Agent;
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{
    AppAsyncExt, AsyncContext, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt,
};
use config_plugin::{MargatroidConfig, WebSocketMessageTarget};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use futures_util::{FutureExt, StreamExt};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, Message, ResourceId, TokenUsage, ToolCall,
    ToolDefinition,
};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use server_plugin::{
    WebSocketConnections, WebSocketMessage, WebSocketMessageSender, WebSocketSender,
};
use tokio::sync::watch;

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
    ResponseEncodeFailed,
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
    context_window_tokens: u64,
    parameters: InferenceParameters,
    workspace: Entity,
    source_image: Entity,
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
    model: String,
    context_window_tokens: u64,
    adapter: ErasedProviderAdapter,
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
    thinking: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
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

pub use margatroid_types::InferenceRequestEvent;

#[derive(Clone, Debug)]
pub struct ContextCompactionInferenceRequest {
    pub id: String,
    pub agent: Entity,
    pub agent_id: ResourceId,
    pub messages: Vec<Message>,
}

impl Event for ContextCompactionInferenceRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCompactionInferenceResponse {
    pub id: String,
    pub agent: Entity,
    pub result: Result<String, InferenceError>,
}

impl Event for ContextCompactionInferenceResponse {}

pub use margatroid_types::{CapturedInferenceRequest, CapturedInferenceResponse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelInferenceRequest {
    pub id: String,
    pub agent: Entity,
}

impl Event for CancelInferenceRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InferenceRoute {
    id: String,
    agent: Entity,
    output: InferenceOutputKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InferenceOutputKind {
    AgentMessage,
    ContextCompaction,
    Captured,
}

struct InferenceCommand {
    id: String,
    agent: Entity,
    agent_id: ResourceId,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    output: InferenceOutputKind,
}

struct PreparedInference {
    route: InferenceRoute,
    agent_id: ResourceId,
    client: reqwest::Client,
    request: ProviderHttpRequest,
    adapter: ErasedProviderAdapter,
    senders: Vec<WebSocketSender>,
    cancellation: watch::Receiver<bool>,
}

impl Event for PreparedInference {}

#[derive(Clone)]
struct InferenceTaskOutput {
    route: InferenceRoute,
    result: InferenceTaskResult,
}

#[derive(Clone)]
enum InferenceTaskResult {
    Completed(Result<ProviderInferenceResponse, InferenceError>),
    Cancelled,
}

#[derive(Default)]
struct InFlightInferences {
    requests: HashMap<(Entity, String), watch::Sender<bool>>,
}

impl Resource for InFlightInferences {}

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
            .map_err(|error| {
                InferenceError::new(
                    InferenceErrorKind::RequestBuildFailed,
                    format!(
                        "inference HTTP client could not be created: {}",
                        summarize_reqwest_error(&error)
                    ),
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
        adapter_factories.insert("deepseek".into(), Arc::new(DeepSeekAdapterFactory));
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
        app.world_mut()
            .insert_resource(WorkspaceModelRoutesRegistry::default());
        app.world_mut()
            .insert_resource(InFlightInferences::default());
        app.add_system(&schedule, reload_model_routes_system)
            .add_system(&schedule, cancel_inference_system)
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
    thinking: Option<String>,
    reasoning_effort: Option<String>,
    context_window: Option<String>,
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
    let mut commands = world
        .event_reader::<InferenceRequestEvent>()
        .into_iter()
        .cloned()
        .map(|command| InferenceCommand {
            id: command.id,
            agent: command.agent,
            agent_id: command.agent_id,
            messages: command.messages,
            tools: command.tools,
            output: InferenceOutputKind::AgentMessage,
        })
        .collect::<Vec<_>>();
    commands.extend(
        world
            .event_reader::<ContextCompactionInferenceRequest>()
            .into_iter()
            .cloned()
            .map(|command| InferenceCommand {
                id: command.id,
                agent: command.agent,
                agent_id: command.agent_id,
                messages: command.messages,
                tools: Vec::new(),
                output: InferenceOutputKind::ContextCompaction,
            }),
    );
    commands.extend(
        world
            .event_reader::<CapturedInferenceRequest>()
            .into_iter()
            .cloned()
            .map(|command| InferenceCommand {
                id: command.id,
                agent: command.agent,
                agent_id: command.agent_id,
                messages: command.messages,
                tools: Vec::new(),
                output: InferenceOutputKind::Captured,
            }),
    );
    let events = world.event_sender();
    for command in commands {
        match prepare_inference(world, command) {
            Ok(prepared) => world.send_async_event(prepared),
            Err((route, error)) => publish_inference_error(&events, route, error),
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
    let request = model_route
        .adapter
        .build_request(ProviderInput::new(
            &model_route.model,
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
        adapter: model_route.adapter,
        senders,
        cancellation,
    })
}

fn cancel_inference_system(world: &mut World) {
    let cancellations = world
        .event_reader::<CancelInferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let in_flight = world
        .get_resource_mut::<InFlightInferences>()
        .expect("InferencePlugin is installed");
    for cancellation in cancellations {
        if let Some(sender) = in_flight
            .requests
            .get(&(cancellation.agent, cancellation.id))
        {
            sender.send_replace(true);
        }
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

async fn execute_prepared_inference(
    prepared: PreparedInference,
    _context: AsyncContext,
) -> Result<InferenceTaskOutput, InferenceTaskError> {
    let route = prepared.route.clone();
    let mut cancellation = prepared.cancellation.clone();
    let provider = std::panic::AssertUnwindSafe(run_provider(prepared)).catch_unwind();
    let result = tokio::select! {
        biased;
        _ = cancellation.changed() => InferenceTaskResult::Cancelled,
        result = provider => InferenceTaskResult::Completed(result.unwrap_or_else(|_| {
            Err(InferenceError::new(
                InferenceErrorKind::TaskPanicked,
                "inference provider task panicked",
            ))
        })),
    };
    Ok(InferenceTaskOutput { route, result })
}

async fn run_provider(
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

fn summarize_reqwest_error(error: &reqwest::Error) -> String {
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

fn publish_inference_output_system(world: &mut World) {
    let mut outputs = Vec::new();
    for output in world.event_reader::<Result<InferenceTaskOutput, InferenceTaskError>>() {
        match output {
            Ok(output) => outputs.push(output.clone()),
            Err(error) => tracing::warn!(error = %error.source, "inference task was cancelled"),
        }
    }
    let events = world.event_sender();
    for output in outputs {
        let cancelled = world
            .get_resource_mut::<InFlightInferences>()
            .expect("InferencePlugin is installed")
            .requests
            .remove(&(output.route.agent, output.route.id.clone()))
            .is_some_and(|sender| *sender.borrow());
        if cancelled || matches!(output.result, InferenceTaskResult::Cancelled) {
            continue;
        }
        let InferenceTaskResult::Completed(result) = &output.result else {
            unreachable!();
        };
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
                        continue;
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
                    events.send_event(margatroid_types::CapturedInferenceResponse {
                        id: output.route.id.clone(),
                        agent: output.route.agent,
                        result: result.map_err(|error| error.to_string()),
                    });
                }
            },
            Err(error) => publish_inference_error(&events, output.route.clone(), error.clone()),
        }
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

fn publish_inference_error(
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
            self.usage = Some(TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_hit_tokens: usage
                    .prompt_tokens_details
                    .or(usage.input_tokens_details)
                    .map_or_else(
                        || usage.prompt_cache_hit_tokens.unwrap_or(0),
                        |details| details.cached_tokens,
                    ),
            });
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
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use std::collections::HashMap;
    use tempfile::tempdir;

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
                input_schema: serde_json::json!({"type":"object"}),
            },
            ToolDefinition {
                name: "skill1_commit".into(),
                description: "Commit code.".into(),
                input_schema: serde_json::json!({"type":"object"}),
            },
        ];
        validate_tools(&tools).unwrap();
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
        app.world().send_event(InferenceRequestEvent {
            id: "request".into(),
            agent,
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            messages: vec![Message::User {
                content: "hello".into(),
            }],
            tools: Vec::new(),
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

        app.world().send_event(ContextCompactionInferenceRequest {
            id: "compact".into(),
            agent,
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            messages: vec![Message::User {
                content: "summarize".into(),
            }],
        });
        app.tick();
        app.tick();
        let response = app
            .world()
            .event_reader::<ContextCompactionInferenceResponse>()
            .into_iter()
            .find(|event| event.id == "compact")
            .unwrap();
        assert_eq!(
            response.result.as_ref().unwrap_err().kind(),
            InferenceErrorKind::InferenceSnapshotMissing
        );
        assert!(app
            .world()
            .event_reader::<AgentMessage>()
            .into_iter()
            .all(|message| message.id != "compact"));
    }
}
