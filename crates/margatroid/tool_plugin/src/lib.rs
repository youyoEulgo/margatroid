use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use async_runtime_plugin::AsyncContext;
use core_plugin::{App, Entity, Event, Plugin, Resource, World};
use margatroid_types::{ResourceName, ToolDefinition};
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolErrorKind {
    InvalidDefinition,
    ToolPluginMissing,
    ToolAlreadyRegistered,
    InvalidArguments,
    ExecutionFailed,
    OutputLimitExceeded,
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
    #[allow(dead_code)]
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

pub struct ToolPlugin;

impl ToolPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for ToolPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<ToolRegistry>() {
            ToolError::new(
                ToolErrorKind::ToolAlreadyRegistered,
                "ToolPlugin is already installed",
            )
            .panic();
        }
        app.world_mut().insert_resource(ToolRegistry::new());
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
}

impl WorldToolExt for World {
    fn registered_tool(&self, name: &ResourceName) -> Option<Tool> {
        self.get_resource::<ToolRegistry>()?.get(name)
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

#[allow(dead_code)]
trait ErasedToolHandler: Send + Sync + 'static {
    fn call(
        &self,
        context: ToolContext,
        arguments: String,
        maximum_output_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>;
}

struct TypedToolHandler<Arguments, Handler> {
    #[allow(dead_code)]
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
            |_context: ToolContext, arguments: AddArguments| async move {
                Ok::<_, Infallible>((arguments.left + arguments.right).to_string())
            },
        )
        .unwrap()
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(ToolPlugin::new());
        app
    }

    #[test]
    fn registered_tool_exposes_its_definition() {
        let mut app = test_app();
        app.register_tool(add_tool("builtin/add", "add"));
        let tool = app
            .world()
            .registered_tool(&ResourceName::new("builtin/add").unwrap())
            .unwrap();
        assert_eq!(tool.definition().name, "add");
    }

    #[test]
    fn invalid_definitions_are_rejected() {
        let error = match Tool::new(
            ResourceName::new("builtin/invalid").unwrap(),
            ToolDefinition {
                name: "invalid name".into(),
                description: "Invalid exposed name".into(),
                input_schema: json!({ "type": "object" }),
            },
            |_context: ToolContext, _arguments: serde_json::Value| async {
                Ok::<_, Infallible>(String::new())
            },
        ) {
            Ok(_) => panic!("invalid definition must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ToolErrorKind::InvalidDefinition);
    }
}
