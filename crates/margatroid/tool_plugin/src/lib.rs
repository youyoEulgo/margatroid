use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{
    AppAsyncExt, AsyncContext, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt,
};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use futures_util::FutureExt;
use margatroid_types::{
    AgentMessage, Message, ResourceName, ResourceRef, ToolCall, ToolDefinition,
};
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    InvalidDefinition,
    DuplicateProvider,
    ProviderMissing,
    ResourceResolutionFailed,
    AgentNotAlive,
    ToolEnvironmentMissing,
    ToolPluginMissing,
    ToolAlreadyRegistered,
    InvalidRequest,
    InvalidArguments,
    ExecutionFailed,
    OutputLimitExceeded,
    TaskPanicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        const MAX_MESSAGE_BYTES: usize = 512;
        const SUFFIX: &str = "...";

        let mut message = message.into();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut boundary = MAX_MESSAGE_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolError {}

pub trait ToolDefinitionProvider: Send + Sync + 'static {
    fn id(&self) -> &str;

    fn provide(
        &self,
        environment: &AgentToolEnvironment,
        name: &ResourceName,
    ) -> Result<Tool, ToolError>;
}

#[derive(Clone)]
pub struct AgentToolEnvironment {
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
}

impl AgentToolEnvironment {
    pub fn new(project_root: impl Into<PathBuf>, image_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Arc::new(project_root.into()),
            image_root: Arc::new(image_root.into()),
        }
    }

    pub fn project_root(&self) -> &Path {
        self.project_root.as_path()
    }

    pub fn image_root(&self) -> &Path {
        self.image_root.as_path()
    }
}

impl Component for AgentToolEnvironment {}

#[derive(Clone)]
pub struct Tool {
    resource: ResourceRef,
    definition: ToolDefinition,
    handler: Arc<dyn ErasedToolHandler>,
}

impl Tool {
    pub fn new<Arguments, Handler, HandlerFuture, Error>(
        resource: ResourceRef,
        definition: ToolDefinition,
        handler: Handler,
    ) -> Result<Self, ToolError>
    where
        Arguments: DeserializeOwned + Send + 'static,
        Handler: Fn(ToolContext, Arguments) -> HandlerFuture + Send + Sync + 'static,
        HandlerFuture: Future<Output = Result<String, Error>> + Send + 'static,
        Error: fmt::Display + Send + Sync + 'static,
    {
        validate_tool(&resource, &definition)?;
        Ok(Self {
            resource,
            definition,
            handler: Arc::new(TypedToolHandler::<Arguments, Handler> {
                handler,
                marker: PhantomData,
            }),
        })
    }

    pub fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub fn definition(&self) -> &ToolDefinition {
        &self.definition
    }
}

#[derive(Clone)]
pub struct ToolContext {
    request_id: Arc<str>,
    agent: Entity,
    tool_call_id: Arc<str>,
    events: AsyncContext,
}

impl ToolContext {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn agent(&self) -> Entity {
        self.agent
    }

    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }

    pub fn send_event<E: Event>(&self, event: E) {
        self.events.send_event(event);
    }

    pub fn send_event_after<E: Event>(&self, event: E, delay: u64) {
        self.events.send_event_after(event, delay);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub id: String,
    pub agent: Entity,
    pub resource: ResourceRef,
    pub call: ToolCall,
}

impl Event for ToolCallRequest {}

pub struct ToolPlugin {
    schedule: String,
}

impl ToolPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ToolPluginInstalled;

impl Resource for ToolPluginInstalled {}

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
        {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "RuntimePlugin and AsyncRuntimePlugin are required",
            )
            .panic();
        }
        if app.world().contains_resource::<ToolPluginInstalled>() {
            ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                "ToolPlugin is already installed",
            )
            .panic();
        }
        if !app.contains_schedule(&self.schedule) {
            ToolError::new(
                ToolErrorKind::InvalidRequest,
                "ToolPlugin schedule does not exist",
            )
            .panic();
        }

        let schedule = self.schedule;
        app.world_mut().insert_resource(ToolPluginInstalled);
        app.world_mut().insert_resource(ToolProviderRegistry::new());
        app.world_mut().insert_resource(ToolState::default());
        app.add_system(&schedule, prepare_tool_call_system)
            .add_async_system(&schedule, execute_tool)
            .add_system(&schedule, publish_tool_call_system);
    }
}

pub trait AppToolExt {
    fn register_tool_provider<P: ToolDefinitionProvider>(&mut self, provider: P) -> &mut Self;
    fn register_tool(&mut self, tool: Tool) -> &mut Self;
}

impl AppToolExt for App {
    fn register_tool_provider<P: ToolDefinitionProvider>(&mut self, provider: P) -> &mut Self {
        let registry = self
            .world_mut()
            .get_resource_mut::<ToolProviderRegistry>()
            .unwrap_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolPluginMissing,
                    "ToolPlugin is not installed",
                )
                .panic()
            });
        registry
            .insert_provider(provider)
            .unwrap_or_else(|error| error.panic());
        self
    }

    fn register_tool(&mut self, tool: Tool) -> &mut Self {
        if tool.resource().provider() != "tool" {
            ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "registered static tools must use provider `tool`",
            )
            .panic();
        }
        let registry = self
            .world_mut()
            .get_resource_mut::<ToolProviderRegistry>()
            .unwrap_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolPluginMissing,
                    "ToolPlugin is not installed",
                )
                .panic()
            });
        registry
            .insert_static_tool(tool)
            .unwrap_or_else(|error| error.panic());
        self
    }
}

pub trait WorldToolExt {
    fn registered_tool(&self, name: &ResourceName) -> Option<Tool>;
    fn resolve_tool(&self, agent: Entity, resource: &ResourceRef) -> Result<Tool, ToolError>;
}

impl WorldToolExt for World {
    fn registered_tool(&self, name: &ResourceName) -> Option<Tool> {
        self.get_resource::<ToolProviderRegistry>()?
            .static_tools
            .get(name)
            .cloned()
    }

    fn resolve_tool(&self, agent: Entity, resource: &ResourceRef) -> Result<Tool, ToolError> {
        if !self.is_alive(agent) {
            return Err(ToolError::new(
                ToolErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ));
        }
        let environment = self
            .get_component::<AgentToolEnvironment>(agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent does not have a tool environment",
                )
            })?;
        let registry = self.get_resource::<ToolProviderRegistry>().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "ToolPlugin is not installed",
            )
        })?;
        let tool = if resource.provider() == "tool" {
            registry
                .static_tools
                .get(resource.name())
                .cloned()
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::ResourceResolutionFailed,
                        "registered tool resource was not found",
                    )
                })?
        } else {
            let provider = registry.providers.get(resource.provider()).ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ProviderMissing,
                    "resource provider was not registered",
                )
            })?;
            provider.provide(environment, resource.name())?
        };
        if tool.resource() != resource {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "tool provider returned a different resource",
            ));
        }
        Ok(tool)
    }
}

struct ToolProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ToolDefinitionProvider>>,
    static_tools: BTreeMap<ResourceName, Tool>,
}

impl ToolProviderRegistry {
    fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
            static_tools: BTreeMap::new(),
        }
    }

    fn insert_provider<P: ToolDefinitionProvider>(&mut self, provider: P) -> Result<(), ToolError> {
        let id = provider.id().to_owned();
        if !is_valid_provider_id(&id) {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "tool provider ID is invalid",
            ));
        }
        if self.providers.contains_key(&id) || id == "tool" {
            return Err(ToolError::new(
                ToolErrorKind::DuplicateProvider,
                "tool provider ID is already registered",
            ));
        }
        self.providers.insert(id, Arc::new(provider));
        Ok(())
    }

    fn insert_static_tool(&mut self, tool: Tool) -> Result<(), ToolError> {
        if self.static_tools.contains_key(tool.resource().name()) {
            return Err(ToolError::new(
                ToolErrorKind::InvalidDefinition,
                "static tool resource is already registered",
            ));
        }
        self.static_tools
            .insert(tool.resource().name().clone(), tool);
        Ok(())
    }
}

impl Resource for ToolProviderRegistry {}

#[derive(Clone, Copy)]
struct ToolState {
    maximum_arguments_bytes: usize,
    maximum_output_bytes: usize,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            maximum_arguments_bytes: 1024 * 1024,
            maximum_output_bytes: 4 * 1024 * 1024,
        }
    }
}

impl Resource for ToolState {}

trait ErasedToolHandler: Send + Sync + 'static {
    fn call(
        &self,
        context: ToolContext,
        arguments: String,
        maximum_output_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>;
}

struct TypedToolHandler<Arguments, Handler> {
    handler: Handler,
    marker: PhantomData<fn() -> Arguments>,
}

impl<Arguments, Handler, HandlerFuture, Error> ErasedToolHandler
    for TypedToolHandler<Arguments, Handler>
where
    Arguments: DeserializeOwned + Send + 'static,
    Handler: Fn(ToolContext, Arguments) -> HandlerFuture + Send + Sync + 'static,
    HandlerFuture: Future<Output = Result<String, Error>> + Send + 'static,
    Error: fmt::Display + Send + Sync + 'static,
{
    fn call(
        &self,
        context: ToolContext,
        arguments: String,
        maximum_output_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>> {
        let arguments = match serde_json::from_str::<Arguments>(&arguments) {
            Ok(arguments) => arguments,
            Err(_) => {
                return Box::pin(async {
                    Err(ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "tool arguments do not match the registered argument type",
                    ))
                });
            }
        };
        let future = (self.handler)(context, arguments);
        Box::pin(async move {
            let output = future.await.map_err(|error| {
                ToolError::new(ToolErrorKind::ExecutionFailed, error.to_string())
            })?;
            if output.len() > maximum_output_bytes {
                return Err(ToolError::new(
                    ToolErrorKind::OutputLimitExceeded,
                    "tool output exceeds the configured byte limit",
                ));
            }
            Ok(output)
        })
    }
}

struct ToolExecutionTask {
    id: String,
    agent: Entity,
    tool_call_id: String,
    handler: Arc<dyn ErasedToolHandler>,
    arguments: String,
    maximum_output_bytes: usize,
}

impl Event for ToolExecutionTask {}

struct ToolExecutionOutput {
    id: String,
    agent: Entity,
    tool_call_id: String,
    result: Result<String, ToolError>,
}

struct ToolTaskError(AsyncTaskError);

impl From<AsyncTaskError> for ToolTaskError {
    fn from(error: AsyncTaskError) -> Self {
        Self(error)
    }
}

async fn execute_tool(
    task: ToolExecutionTask,
    context: AsyncContext,
) -> Result<ToolExecutionOutput, ToolTaskError> {
    let ToolExecutionTask {
        id,
        agent,
        tool_call_id,
        handler,
        arguments,
        maximum_output_bytes,
    } = task;
    let tool_context = ToolContext {
        request_id: Arc::from(id.as_str()),
        agent,
        tool_call_id: Arc::from(tool_call_id.as_str()),
        events: context,
    };
    let execution = std::panic::AssertUnwindSafe(async move {
        handler
            .call(tool_context, arguments, maximum_output_bytes)
            .await
    })
    .catch_unwind()
    .await;
    let result = match execution {
        Ok(result) => result,
        Err(_) => Err(ToolError::new(
            ToolErrorKind::TaskPanicked,
            "tool handler panicked",
        )),
    };
    Ok(ToolExecutionOutput {
        id,
        agent,
        tool_call_id,
        result,
    })
}

fn prepare_tool_call_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let limits = *world
        .get_resource::<ToolState>()
        .expect("ToolPlugin is not installed");

    for request in requests {
        let invalid = if request.id.is_empty()
            || request.call.id.is_empty()
            || request.call.name.is_empty()
        {
            Some(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "request ID, tool call ID, and tool name must be non-empty",
            ))
        } else if request.call.arguments.len() > limits.maximum_arguments_bytes {
            Some(ToolError::new(
                ToolErrorKind::InvalidArguments,
                "tool arguments exceed the configured byte limit",
            ))
        } else {
            None
        };
        if let Some(error) = invalid {
            send_tool_message(
                world,
                request.id,
                request.agent,
                request.call.id,
                error.to_string(),
            );
            continue;
        }

        let tool = match world.resolve_tool(request.agent, &request.resource) {
            Ok(tool) => tool,
            Err(error) => {
                send_tool_message(
                    world,
                    request.id,
                    request.agent,
                    request.call.id,
                    error.to_string(),
                );
                continue;
            }
        };
        if tool.definition().name != request.call.name {
            send_tool_message(
                world,
                request.id,
                request.agent,
                request.call.id,
                ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "tool call name does not match the resolved tool",
                )
                .to_string(),
            );
            continue;
        }

        world.send_async_event(ToolExecutionTask {
            id: request.id,
            agent: request.agent,
            tool_call_id: request.call.id,
            handler: Arc::clone(&tool.handler),
            arguments: request.call.arguments,
            maximum_output_bytes: limits.maximum_output_bytes,
        });
    }
}

fn publish_tool_call_system(world: &mut World) {
    let outputs = world
        .event_reader::<Result<ToolExecutionOutput, ToolTaskError>>()
        .into_iter()
        .filter_map(|result| match result {
            Ok(output) => {
                let content = match &output.result {
                    Ok(content) => content.clone(),
                    Err(error) => error.to_string(),
                };
                Some((
                    output.id.clone(),
                    output.agent,
                    output.tool_call_id.clone(),
                    content,
                ))
            }
            Err(error) => {
                let _ = &error.0;
                None
            }
        })
        .collect::<Vec<_>>();
    for (id, agent, tool_call_id, content) in outputs {
        send_tool_message(world, id, agent, tool_call_id, content);
    }
}

fn send_tool_message(
    world: &World,
    id: String,
    agent: Entity,
    tool_call_id: String,
    content: String,
) {
    world.send_event(AgentMessage {
        id,
        agent,
        message: Message::Tool {
            tool_call_id,
            content,
        },
    });
}

fn validate_tool(resource: &ResourceRef, definition: &ToolDefinition) -> Result<(), ToolError> {
    let valid_name = !definition.name.is_empty()
        && definition.name.len() <= 64
        && definition
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if !valid_name {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("tool `{}` has an invalid exposed name", resource.name()),
        ));
    }
    if definition.description.trim().is_empty() || definition.description.len() > 8 * 1024 {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("tool `{}` has an invalid description", resource.name()),
        ));
    }
    if !definition.input_schema.is_object() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!(
                "tool `{}` input schema must be a JSON object",
                resource.name()
            ),
        ));
    }
    Ok(())
}

fn is_valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::future::Ready;
    use std::time::{Duration, Instant};

    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use margatroid_types::ResourceName;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize)]
    struct EchoArguments {
        text: String,
    }

    fn resource(value: &str) -> ResourceRef {
        ResourceRef::new("tool", ResourceName::new(value).unwrap()).unwrap()
    }

    fn echo_tool() -> Tool {
        Tool::new(
            resource("builtin/echo"),
            ToolDefinition {
                name: "echo".into(),
                description: "Return text".into(),
                input_schema: json!({"type":"object"}),
            },
            |context: ToolContext, arguments: EchoArguments| async move {
                Ok::<_, Infallible>(format!(
                    "{}:{}:{}",
                    context.request_id(),
                    context.tool_call_id(),
                    arguments.text
                ))
            },
        )
        .unwrap()
    }

    fn panic_before_returning_future(
        _context: ToolContext,
        _arguments: EchoArguments,
    ) -> Ready<Result<String, Infallible>> {
        panic!("synchronous handler panic")
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ToolPlugin::default());
        app
    }

    #[test]
    fn registered_tool_resolves_with_its_resource() {
        let mut app = test_app();
        app.register_tool(echo_tool());
        let agent = app.world_mut().spawn();
        app.world_mut()
            .insert_component(agent, AgentToolEnvironment::new("/project", "/image"));
        let tool = app
            .world()
            .resolve_tool(agent, &resource("builtin/echo"))
            .unwrap();
        assert_eq!(tool.resource(), &resource("builtin/echo"));
        assert_eq!(tool.definition().name, "echo");
    }

    #[test]
    fn tool_request_produces_agent_message() {
        let mut app = test_app();
        app.register_tool(echo_tool());
        let agent = app.world_mut().spawn();
        app.world_mut()
            .insert_component(agent, AgentToolEnvironment::new("/project", "/image"));
        app.world().send_event(ToolCallRequest {
            id: "request".into(),
            agent,
            resource: resource("builtin/echo"),
            call: ToolCall {
                id: "call".into(),
                name: "echo".into(),
                arguments: r#"{"text":"hello"}"#.into(),
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(message) = app
                .world()
                .event_reader::<AgentMessage>()
                .into_iter()
                .next()
            {
                assert_eq!(message.id, "request");
                assert_eq!(message.agent, agent);
                assert_eq!(
                    message.message,
                    Message::Tool {
                        tool_call_id: "call".into(),
                        content: "request:call:hello".into(),
                    }
                );
                break;
            }
            assert!(Instant::now() < deadline, "tool execution timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn synchronous_handler_panic_produces_tool_message() {
        let mut app = test_app();
        let tool = Tool::new(
            resource("builtin/panic"),
            ToolDefinition {
                name: "panic".into(),
                description: "Panic before returning a future".into(),
                input_schema: json!({"type":"object"}),
            },
            panic_before_returning_future,
        )
        .unwrap();
        app.register_tool(tool);
        let agent = app.world_mut().spawn();
        app.world_mut()
            .insert_component(agent, AgentToolEnvironment::new("/project", "/image"));
        app.world().send_event(ToolCallRequest {
            id: "request".into(),
            agent,
            resource: resource("builtin/panic"),
            call: ToolCall {
                id: "call".into(),
                name: "panic".into(),
                arguments: r#"{"text":"hello"}"#.into(),
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(message) = app
                .world()
                .event_reader::<AgentMessage>()
                .into_iter()
                .next()
            {
                assert_eq!(
                    message.message,
                    Message::Tool {
                        tool_call_id: "call".into(),
                        content: "TaskPanicked: tool handler panicked".into(),
                    }
                );
                break;
            }
            assert!(Instant::now() < deadline, "tool execution timed out");
            std::thread::yield_now();
        }
    }
}
