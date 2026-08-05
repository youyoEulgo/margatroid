use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{
    AppAsyncExt, AsyncContext, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt,
};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use futures_util::FutureExt;
use inference_plugin::{AgentToolDefinitions, ToolCall, ToolDefinition};
use margatroid_types::ResourceName;
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    InvalidDefinition,
    DuplicateTool,
    DuplicateExposedName,
    ToolPluginMissing,
    ToolAlreadyRegistered,
    ToolNotRegistered,
    InvalidRequest,
    AgentNotAlive,
    ToolCatalogMissing,
    ToolNotVisible,
    InvalidArguments,
    ExecutionFailed,
    OutputLimitExceeded,
    TaskPanicked,
}

#[derive(Clone, Debug)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

impl ToolError {
    fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
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

#[derive(Clone)]
pub struct Tool {
    name: ResourceName,
    definition: ToolDefinition,
    handler: Arc<dyn ErasedToolHandler>,
}

impl Tool {
    pub fn new<Arguments, Handler, HandlerFuture, Error>(
        name: ResourceName,
        definition: ToolDefinition,
        handler: Handler,
    ) -> Result<Self, ToolError>
    where
        Arguments: DeserializeOwned + Send + 'static,
        Handler: Fn(ToolContext, Arguments) -> HandlerFuture + Send + Sync + 'static,
        HandlerFuture: Future<Output = Result<String, Error>> + Send + 'static,
        Error: fmt::Display + Send + Sync + 'static,
    {
        validate_tool(&name, &definition)?;
        Ok(Self {
            name,
            definition,
            handler: Arc::new(TypedToolHandler::<Arguments, Handler> {
                handler,
                marker: PhantomData,
            }),
        })
    }

    pub fn name(&self) -> &ResourceName {
        &self.name
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

pub struct AgentToolCatalog {
    tools: BTreeMap<String, Tool>,
}

impl AgentToolCatalog {
    pub fn new(tools: impl IntoIterator<Item = Tool>) -> Result<Self, ToolError> {
        let mut logical_names = BTreeSet::new();
        let mut catalog = BTreeMap::new();
        for tool in tools {
            if !logical_names.insert(tool.name.clone()) {
                return Err(ToolError::new(
                    ToolErrorKind::DuplicateTool,
                    format!("tool `{}` appears more than once", tool.name),
                ));
            }
            let exposed_name = tool.definition.name.clone();
            if catalog.insert(exposed_name.clone(), tool).is_some() {
                return Err(ToolError::new(
                    ToolErrorKind::DuplicateExposedName,
                    format!("exposed tool name `{exposed_name}` appears more than once"),
                ));
            }
        }
        Ok(Self { tools: catalog })
    }

    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> + '_ {
        self.tools.values().map(Tool::definition)
    }

    pub fn contains(&self, name: &ResourceName) -> bool {
        self.tools.values().any(|tool| tool.name() == name)
    }

    pub(crate) fn get(&self, exposed_name: &str) -> Option<&Tool> {
        self.tools.get(exposed_name)
    }
}

impl Component for AgentToolCatalog {}

pub struct ToolCallCommand {
    pub id: String,
    pub agent: Entity,
    pub call: ToolCall,
}

impl Event for ToolCallCommand {}

pub struct ToolCallResult {
    pub id: String,
    pub agent: Entity,
    pub tool_call_id: String,
    pub result: Result<String, ToolError>,
}

impl Event for ToolCallResult {}

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

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "RuntimePlugin is not installed",
            )
            .panic();
        }
        if !app.world().contains_resource::<AsyncRuntimeHandle>() {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "AsyncRuntimePlugin is not installed",
            )
            .panic();
        }
        if app.world().contains_resource::<ToolRegistry>() {
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
        app.world_mut().insert_resource(ToolRegistry::new());
        app.world_mut().insert_resource(ToolState {
            limits: ToolLimits::default(),
        });
        app.add_system(&schedule, prepare_tool_call_system)
            .add_async_system(&schedule, execute_tool)
            .add_system(&schedule, publish_tool_call_system);
    }
}

pub trait AppToolExt {
    fn register_tool(&mut self, tool: Tool) -> &mut Self;
}

impl AppToolExt for App {
    fn register_tool(&mut self, tool: Tool) -> &mut Self {
        let registry = self
            .world_mut()
            .get_resource_mut::<ToolRegistry>()
            .unwrap_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolPluginMissing,
                    "ToolPlugin is not installed",
                )
                .panic()
            });
        registry.insert(tool).unwrap_or_else(|error| error.panic());
        self
    }
}

pub trait WorldToolExt {
    fn registered_tool(&self, name: &ResourceName) -> Option<Tool>;

    fn registered_tools(
        &self,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<Vec<Tool>, ToolError>;

    fn set_agent_tools(
        &mut self,
        agent: Entity,
        tools: impl IntoIterator<Item = Tool>,
    ) -> Result<(), ToolError>;

    fn set_registered_agent_tools(
        &mut self,
        agent: Entity,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<(), ToolError>;

    fn send_tool_call(&self, id: impl Into<String>, agent: Entity, call: ToolCall);
}

impl WorldToolExt for World {
    fn registered_tool(&self, name: &ResourceName) -> Option<Tool> {
        self.get_resource::<ToolRegistry>()?.get(name)
    }

    fn registered_tools(
        &self,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<Vec<Tool>, ToolError> {
        let registry = self.get_resource::<ToolRegistry>().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ToolPluginMissing,
                "ToolPlugin is not installed",
            )
        })?;
        names
            .into_iter()
            .map(|name| {
                registry.get(&name).ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::ToolNotRegistered,
                        format!("tool `{name}` is not registered"),
                    )
                })
            })
            .collect()
    }

    fn set_agent_tools(
        &mut self,
        agent: Entity,
        tools: impl IntoIterator<Item = Tool>,
    ) -> Result<(), ToolError> {
        if !self.is_alive(agent) {
            return Err(ToolError::new(
                ToolErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ));
        }
        let catalog = AgentToolCatalog::new(tools)?;
        let definitions = AgentToolDefinitions::new(catalog.definitions().cloned().collect());
        assert!(self.insert_component(agent, catalog));
        assert!(self.insert_component(agent, definitions));
        Ok(())
    }

    fn set_registered_agent_tools(
        &mut self,
        agent: Entity,
        names: impl IntoIterator<Item = ResourceName>,
    ) -> Result<(), ToolError> {
        let tools = self.registered_tools(names)?;
        self.set_agent_tools(agent, tools)
    }

    fn send_tool_call(&self, id: impl Into<String>, agent: Entity, call: ToolCall) {
        WorldEventExt::send_event(
            self,
            ToolCallCommand {
                id: id.into(),
                agent,
                call,
            },
        );
    }
}

pub(crate) struct ToolRegistry {
    tools: BTreeMap<ResourceName, Tool>,
}

impl ToolRegistry {
    pub(crate) fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, tool: Tool) -> Result<(), ToolError> {
        if self.tools.contains_key(tool.name()) {
            return Err(ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                format!("tool `{}` is already registered", tool.name()),
            ));
        }
        self.tools.insert(tool.name.clone(), tool);
        Ok(())
    }

    pub(crate) fn get(&self, name: &ResourceName) -> Option<Tool> {
        self.tools.get(name).cloned()
    }
}

impl Resource for ToolRegistry {}

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
    arguments: String,
    handler: Arc<dyn ErasedToolHandler>,
    maximum_output_bytes: usize,
}

impl Event for ToolExecutionTask {}

struct ToolExecutionPayload {
    id: String,
    agent: Entity,
    tool_call_id: String,
    result: Result<String, ToolError>,
}

struct ToolExecutionOutput {
    payload: Mutex<Option<ToolExecutionPayload>>,
}

impl ToolExecutionOutput {
    fn new(payload: ToolExecutionPayload) -> Self {
        Self {
            payload: Mutex::new(Some(payload)),
        }
    }

    fn take(&self) -> Option<ToolExecutionPayload> {
        self.payload
            .lock()
            .expect("tool execution output lock poisoned")
            .take()
    }
}

struct ToolTaskError {
    source: AsyncTaskError,
}

impl From<AsyncTaskError> for ToolTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

#[derive(Clone, Copy)]
struct ToolLimits {
    maximum_arguments_bytes: usize,
    maximum_output_bytes: usize,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            maximum_arguments_bytes: 1024 * 1024,
            maximum_output_bytes: 4 * 1024 * 1024,
        }
    }
}

struct ToolState {
    limits: ToolLimits,
}

impl Resource for ToolState {}

fn prepare_tool_call_system(world: &mut World) {
    let commands = world
        .event_reader::<ToolCallCommand>()
        .into_iter()
        .map(|command| (command.id.clone(), command.agent, command.call.clone()))
        .collect::<Vec<_>>();
    let limits = world
        .get_resource::<ToolState>()
        .expect("ToolPlugin is not installed")
        .limits;

    for (id, agent, call) in commands {
        let invalid = if id.is_empty() || call.id.is_empty() || call.name.is_empty() {
            Some(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "request id, tool call id, and tool name must be non-empty",
            ))
        } else if !world.is_alive(agent) {
            Some(ToolError::new(
                ToolErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ))
        } else if call.arguments.len() > limits.maximum_arguments_bytes {
            Some(ToolError::new(
                ToolErrorKind::InvalidArguments,
                "tool arguments exceed the configured byte limit",
            ))
        } else {
            None
        };
        if let Some(error) = invalid {
            send_tool_result(world, id, agent, call.id, Err(error));
            continue;
        }

        let Some(catalog) = world.get_component::<AgentToolCatalog>(agent) else {
            send_tool_result(
                world,
                id,
                agent,
                call.id,
                Err(ToolError::new(
                    ToolErrorKind::ToolCatalogMissing,
                    "agent does not have a tool catalog",
                )),
            );
            continue;
        };
        let Some(tool) = catalog.get(&call.name) else {
            send_tool_result(
                world,
                id,
                agent,
                call.id,
                Err(ToolError::new(
                    ToolErrorKind::ToolNotVisible,
                    "tool is not visible to the agent",
                )),
            );
            continue;
        };
        let task = ToolExecutionTask {
            id,
            agent,
            tool_call_id: call.id,
            arguments: call.arguments,
            handler: Arc::clone(&tool.handler),
            maximum_output_bytes: limits.maximum_output_bytes,
        };
        world.send_async_event(task);
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
        arguments,
        handler,
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
    Ok(ToolExecutionOutput::new(ToolExecutionPayload {
        id,
        agent,
        tool_call_id,
        result,
    }))
}

fn publish_tool_call_system(world: &mut World) {
    let mut payloads = Vec::new();
    for output in world.event_reader::<Result<ToolExecutionOutput, ToolTaskError>>() {
        match output {
            Ok(output) => payloads.extend(output.take()),
            Err(error) => tracing::error!(error = %error.source, "tool async task stopped"),
        }
    }
    for payload in payloads {
        send_tool_result(
            world,
            payload.id,
            payload.agent,
            payload.tool_call_id,
            payload.result,
        );
    }
}

fn send_tool_result(
    world: &World,
    id: String,
    agent: Entity,
    tool_call_id: String,
    result: Result<String, ToolError>,
) {
    WorldEventExt::send_event(
        world,
        ToolCallResult {
            id,
            agent,
            tool_call_id,
            result,
        },
    );
}

fn validate_tool(name: &ResourceName, definition: &ToolDefinition) -> Result<(), ToolError> {
    let valid_name = !definition.name.is_empty()
        && definition.name.len() <= 64
        && definition
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if !valid_name {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("tool `{name}` has an invalid exposed name"),
        ));
    }
    if definition.description.trim().is_empty() || definition.description.len() > 8 * 1024 {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("tool `{name}` has an invalid description"),
        ));
    }
    if !definition.input_schema.is_object() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("tool `{name}` input schema must be a JSON object"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::future::Ready;
    use std::time::{Duration, Instant};

    use async_runtime_plugin::AsyncRuntimePlugin;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize)]
    struct AddArguments {
        left: i64,
        right: i64,
    }

    fn add_tool(logical_name: &str, exposed_name: &str) -> Tool {
        Tool::new(
            ResourceName::new(logical_name).unwrap(),
            ToolDefinition {
                name: exposed_name.into(),
                description: "Add two integers".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "left": { "type": "integer" },
                        "right": { "type": "integer" }
                    },
                    "required": ["left", "right"]
                }),
            },
            |context: ToolContext, arguments: AddArguments| async move {
                assert!(!context.request_id().is_empty());
                Ok::<_, Infallible>((arguments.left + arguments.right).to_string())
            },
        )
        .unwrap()
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ToolPlugin::default());
        app
    }

    fn wait_for_result(app: &mut App, id: &str) -> (String, Result<String, ToolError>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(result) = app
                .world()
                .event_reader::<ToolCallResult>()
                .into_iter()
                .find(|result| result.id == id)
            {
                return (result.tool_call_id.clone(), result.result.clone());
            }
            assert!(Instant::now() < deadline, "tool execution timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn registered_tool_executes_for_an_agent_and_preserves_route() {
        let mut app = test_app();
        app.register_tool(add_tool("builtin/add", "add"));
        let agent = app.world_mut().spawn();
        app.world_mut()
            .set_registered_agent_tools(agent, [ResourceName::new("builtin/add").unwrap()])
            .unwrap();

        app.world().send_tool_call(
            "request-1",
            agent,
            ToolCall {
                id: "call-1".into(),
                name: "add".into(),
                arguments: r#"{"left": 2, "right": 5}"#.into(),
            },
        );

        let (tool_call_id, result) = wait_for_result(&mut app, "request-1");
        assert_eq!(tool_call_id, "call-1");
        assert_eq!(result.unwrap(), "7");
    }

    #[test]
    fn agent_definitions_are_projected_in_exposed_name_order() {
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .set_agent_tools(
                agent,
                [
                    add_tool("builtin/zeta", "zeta"),
                    add_tool("builtin/alpha", "alpha"),
                ],
            )
            .unwrap();

        let definitions = app
            .world()
            .get_component::<AgentToolDefinitions>(agent)
            .unwrap();
        assert_eq!(
            definitions
                .tools()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
    }

    #[test]
    fn invisible_and_invalid_arguments_return_stable_errors() {
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .set_agent_tools(agent, std::iter::empty())
            .unwrap();
        app.world().send_tool_call(
            "hidden",
            agent,
            ToolCall {
                id: "hidden-call".into(),
                name: "add".into(),
                arguments: "{}".into(),
            },
        );
        assert_eq!(
            wait_for_result(&mut app, "hidden").1.unwrap_err().kind(),
            ToolErrorKind::ToolNotVisible
        );

        app.register_tool(add_tool("builtin/add", "add"));
        app.world_mut()
            .set_registered_agent_tools(agent, [ResourceName::new("builtin/add").unwrap()])
            .unwrap();
        app.world().send_tool_call(
            "invalid",
            agent,
            ToolCall {
                id: "invalid-call".into(),
                name: "add".into(),
                arguments: "{}".into(),
            },
        );
        assert_eq!(
            wait_for_result(&mut app, "invalid").1.unwrap_err().kind(),
            ToolErrorKind::InvalidArguments
        );
    }

    #[test]
    fn duplicate_catalog_entries_are_rejected_without_replacing_the_old_catalog() {
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .set_agent_tools(agent, [add_tool("builtin/add", "add")])
            .unwrap();

        let error = app
            .world_mut()
            .set_agent_tools(
                agent,
                [
                    add_tool("builtin/first", "same"),
                    add_tool("builtin/second", "same"),
                ],
            )
            .unwrap_err();
        assert_eq!(error.kind(), ToolErrorKind::DuplicateExposedName);
        assert!(app
            .world()
            .get_component::<AgentToolCatalog>(agent)
            .unwrap()
            .contains(&ResourceName::new("builtin/add").unwrap()));
    }

    #[test]
    fn duplicate_logical_names_are_rejected() {
        let error = match AgentToolCatalog::new([
            add_tool("builtin/add", "add_first"),
            add_tool("builtin/add", "add_second"),
        ]) {
            Ok(_) => panic!("duplicate logical names must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ToolErrorKind::DuplicateTool);
    }

    #[test]
    fn handler_panic_becomes_a_routed_tool_error() {
        let mut app = test_app();
        let panic_tool = Tool::new(
            ResourceName::new("builtin/panic").unwrap(),
            ToolDefinition {
                name: "panic_tool".into(),
                description: "Panic while starting".into(),
                input_schema: json!({ "type": "object" }),
            },
            |_context: ToolContext,
             _arguments: serde_json::Value|
             -> Ready<Result<String, Infallible>> { panic!("handler panic") },
        )
        .unwrap();
        app.register_tool(panic_tool);
        let agent = app.world_mut().spawn();
        app.world_mut()
            .set_registered_agent_tools(agent, [ResourceName::new("builtin/panic").unwrap()])
            .unwrap();
        app.world().send_tool_call(
            "panic-request",
            agent,
            ToolCall {
                id: "panic-call".into(),
                name: "panic_tool".into(),
                arguments: "{}".into(),
            },
        );

        let (tool_call_id, result) = wait_for_result(&mut app, "panic-request");
        assert_eq!(tool_call_id, "panic-call");
        assert_eq!(result.unwrap_err().kind(), ToolErrorKind::TaskPanicked);
    }
}
