use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_plugin::Agent;
use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Entity, Event, Plugin, Resource, World};
use futures_util::StreamExt;
use margatroid_types::ResourceId;
use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, VmState};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tool_plugin::{
    candidate_resource_entry, ResourceMapEntry, ToolCallRequest, ToolCallResponse, ToolError,
    ToolErrorKind, ToolPluginInstalled, ToolTemplate,
};

const LUA_RUNTIME_ID: &str = "tool:builtin/lua-runtime:latest";
const TOOL_METADATA_FILE: &str = "tool.toml";
const TOOL_SCHEMA_FILE: &str = "input.schema.json";
const TOOL_SCRIPT_FILE: &str = "main.lua";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaExecutionLimits {
    max_definition_bytes: usize,
    max_script_bytes: usize,
    max_argument_bytes: usize,
    max_output_bytes: usize,
    max_memory_bytes: usize,
    max_instructions: u64,
    max_execution_time: Duration,
    max_host_call_time: Duration,
}

impl LuaExecutionLimits {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_definition_bytes: usize,
        max_script_bytes: usize,
        max_argument_bytes: usize,
        max_output_bytes: usize,
        max_memory_bytes: usize,
        max_instructions: u64,
        max_execution_time: Duration,
        max_host_call_time: Duration,
    ) -> Result<Self, LuaError> {
        if [
            max_definition_bytes,
            max_script_bytes,
            max_argument_bytes,
            max_output_bytes,
            max_memory_bytes,
        ]
        .contains(&0)
            || max_instructions == 0
            || max_execution_time.is_zero()
            || max_host_call_time.is_zero()
            || max_host_call_time > max_execution_time
        {
            return Err(LuaError::new(
                LuaErrorKind::InvalidLimits,
                "Lua execution limits must be nonzero and internally consistent",
            ));
        }
        Ok(Self {
            max_definition_bytes,
            max_script_bytes,
            max_argument_bytes,
            max_output_bytes,
            max_memory_bytes,
            max_instructions,
            max_execution_time,
            max_host_call_time,
        })
    }
}

impl Default for LuaExecutionLimits {
    fn default() -> Self {
        Self::new(
            64 * 1024,
            4 * 1024 * 1024,
            1024 * 1024,
            16 * 1024 * 1024,
            256 * 1024 * 1024,
            100_000_000,
            Duration::from_secs(15 * 60),
            Duration::from_secs(5 * 60),
        )
        .expect("default Lua limits are valid")
    }
}
impl Resource for LuaExecutionLimits {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaErrorKind {
    InvalidRoot,
    InvalidLimits,
    DependencyMissing,
    AlreadyInstalled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaError {
    kind: LuaErrorKind,
    message: String,
}

impl LuaError {
    fn new(kind: LuaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> LuaErrorKind {
        self.kind
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for LuaError {}

pub struct LuaPlugin {
    home_root: Arc<PathBuf>,
    limits: LuaExecutionLimits,
}

impl LuaPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, LuaError> {
        let home_root = normalize_root(home_root.into()).ok_or_else(|| {
            LuaError::new(
                LuaErrorKind::InvalidRoot,
                "Lua tool root must be absolute and cannot contain parent traversal",
            )
        })?;
        Ok(Self {
            home_root: Arc::new(home_root),
            limits: LuaExecutionLimits::default(),
        })
    }

    pub fn with_limits(mut self, limits: LuaExecutionLimits) -> Result<Self, LuaError> {
        self.limits = limits;
        Ok(self)
    }
}

impl Plugin for LuaPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
            || !app.world().contains_resource::<ToolPluginInstalled>()
        {
            panic!("LuaPlugin requires RuntimePlugin, AsyncRuntimePlugin, and ToolPlugin");
        }
        if app.world().contains_resource::<LuaRoots>() {
            panic!("LuaPlugin is already installed");
        }
        let client = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|error| panic!("LuaPlugin HTTP client could not be built: {error}"));
        app.world_mut().insert_resource(LuaRoots {
            home_root: self.home_root,
        });
        app.world_mut().insert_resource(self.limits);
        app.world_mut().insert_resource(LuaHttpClient(client));
        app.add_system(RuntimePlugin::UPDATE, lua_tool_register_system)
            .add_system(RuntimePlugin::UPDATE, lua_tool_call_prepare_system)
            .add_async_system(RuntimePlugin::UPDATE, execute_prepared_lua_tool)
            .add_system(RuntimePlugin::UPDATE, lua_task_result_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaToolRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
}
impl Event for LuaToolRegisterRequest {}

#[derive(Clone, Debug, PartialEq)]
pub struct LuaToolRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub alias: Option<String>,
    pub result: Result<ResourceMapEntry, ToolError>,
}
impl Event for LuaToolRegisterResponse {}

struct LuaRoots {
    home_root: Arc<PathBuf>,
}
impl Resource for LuaRoots {}

struct LuaHttpClient(reqwest::Client);
impl Resource for LuaHttpClient {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LuaToolMetadata {
    schema_version: u32,
    name: String,
    description: String,
}

struct LuaToolDefinition {
    metadata: LuaToolMetadata,
    parameters: serde_json::Value,
}

struct LuaToolPackage {
    #[allow(dead_code)]
    definition: LuaToolDefinition,
    script: String,
}

struct LuaToolCallLocator {
    turn_id: String,
    agent: Entity,
    tool_call_id: String,
}

struct LuaCallContext {
    agent_id: ResourceId,
    turn_id: String,
    resource_id: ResourceId,
    project_root: Arc<PathBuf>,
    image_root: Arc<PathBuf>,
    package_root: Arc<PathBuf>,
}

struct LuaExecutionHandle {
    context: LuaCallContext,
    capabilities: LuaDirectCapabilityHandle,
    limits: LuaExecutionLimits,
}

struct LuaDirectCapabilityHandle {
    fs: LuaFileHandle,
    http: LuaHttpHandle,
    json: LuaJsonHandle,
    log: LuaLogHandle,
    process: LuaProcessHandle,
}

struct LuaFileHandle {
    limits: LuaExecutionLimits,
}

#[derive(Clone)]
struct LuaHttpHandle {
    client: reqwest::Client,
    limits: LuaExecutionLimits,
}

struct LuaJsonHandle;

struct LuaLogHandle {
    agent_id: ResourceId,
    turn_id: String,
    resource_id: ResourceId,
}

#[derive(Clone)]
struct LuaProcessHandle {
    limits: LuaExecutionLimits,
}

struct LuaToolResponseGuard {
    locator: Option<LuaToolCallLocator>,
    events: RuntimeEventSender,
}

impl LuaToolResponseGuard {
    fn new(request: &ToolCallRequest, events: RuntimeEventSender) -> Self {
        Self {
            locator: Some(LuaToolCallLocator {
                turn_id: request.turn_id.clone(),
                agent: request.agent,
                tool_call_id: request.tool_call_id.clone(),
            }),
            events,
        }
    }

    fn respond(&mut self, result: Result<String, ToolError>) {
        let locator = self
            .locator
            .take()
            .expect("Lua tool response was already sent");
        self.events.send_event(ToolCallResponse {
            turn_id: locator.turn_id,
            agent: locator.agent,
            tool_call_id: locator.tool_call_id,
            result,
        });
    }
}

impl Drop for LuaToolResponseGuard {
    fn drop(&mut self) {
        let Some(locator) = self.locator.take() else {
            return;
        };
        self.events.send_event(ToolCallResponse {
            turn_id: locator.turn_id,
            agent: locator.agent,
            tool_call_id: locator.tool_call_id,
            result: Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Lua tool task did not complete",
            )),
        });
    }
}

struct PreparedLuaToolCall {
    package_root: Arc<PathBuf>,
    arguments: String,
    handle: LuaExecutionHandle,
    response: LuaToolResponseGuard,
}
impl Event for PreparedLuaToolCall {}

struct LuaTaskError {
    source: AsyncTaskError,
}
impl From<AsyncTaskError> for LuaTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

fn lua_tool_register_system(world: &mut World) {
    let requests = world
        .event_reader::<LuaToolRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_lua_tool(world, &request);
        world.send_event(LuaToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn register_lua_tool(
    world: &mut World,
    request: &LuaToolRegisterRequest,
) -> Result<ResourceMapEntry, ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Lua tool registration request is invalid",
        ));
    }
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    let home_root = &world
        .get_resource::<LuaRoots>()
        .expect("LuaPlugin is installed")
        .home_root;
    let limits = world
        .get_resource::<LuaExecutionLimits>()
        .expect("LuaPlugin is installed");
    let package = find_lua_tool_package(
        &agent.info.project_root,
        &agent.info.image_root,
        home_root,
        &request.resource_id,
    )?;
    let metadata = read_bounded_sync(
        &package.join(TOOL_METADATA_FILE),
        limits.max_definition_bytes,
        "Lua tool metadata",
    )?;
    let schema = read_bounded_sync(
        &package.join(TOOL_SCHEMA_FILE),
        limits.max_definition_bytes,
        "Lua tool schema",
    )?;
    let definition = parse_lua_tool_definition(&metadata, &schema, &request.resource_id)?;
    let template = ToolTemplate::new(
        request.resource_id.to_string(),
        definition.metadata.description,
        definition.parameters,
    )?;
    candidate_resource_entry(
        request.resource_id.clone(),
        request.alias.clone(),
        ResourceId::parse(LUA_RUNTIME_ID).expect("built-in Lua runtime ID is valid"),
        template,
    )
}

fn lua_tool_call_prepare_system(world: &mut World) {
    let runtime_id = ResourceId::parse(LUA_RUNTIME_ID).expect("built-in Lua runtime ID is valid");
    let requests = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .filter(|request| request.tool_id == runtime_id)
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        match prepare_lua_tool_call(world, &request) {
            Ok((package_root, handle)) => {
                let response = LuaToolResponseGuard::new(&request, world.event_sender());
                world.send_async_event(PreparedLuaToolCall {
                    package_root,
                    arguments: request.arguments,
                    handle,
                    response,
                });
            }
            Err(error) => world.send_event(ToolCallResponse {
                turn_id: request.turn_id,
                agent: request.agent,
                tool_call_id: request.tool_call_id,
                result: Err(error),
            }),
        }
    }
}

fn prepare_lua_tool_call(
    world: &World,
    request: &ToolCallRequest,
) -> Result<(Arc<PathBuf>, LuaExecutionHandle), ToolError> {
    let limits = world
        .get_resource::<LuaExecutionLimits>()
        .expect("LuaPlugin is installed")
        .clone();
    if request.turn_id.is_empty()
        || request.tool_call_id.is_empty()
        || request.resource_id.resource_type() != "tool"
        || request.arguments.len() > limits.max_argument_bytes
        || !world.is_alive(request.agent)
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Lua tool call request is invalid",
        ));
    }
    let agent = world
        .get_component::<Agent>(request.agent)
        .ok_or_else(|| ToolError::new(ToolErrorKind::InvalidRequest, "Agent is missing"))?;
    let agent_id = world
        .get_component::<ResourceId>(request.agent)
        .cloned()
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidRequest,
                "Agent resource id is missing",
            )
        })?;
    let home_root = &world
        .get_resource::<LuaRoots>()
        .expect("LuaPlugin is installed")
        .home_root;
    let package_root = Arc::new(find_lua_tool_package(
        &agent.info.project_root,
        &agent.info.image_root,
        home_root,
        &request.resource_id,
    )?);
    let client = world
        .get_resource::<LuaHttpClient>()
        .expect("LuaPlugin is installed")
        .0
        .clone();
    let context = LuaCallContext {
        agent_id,
        turn_id: request.turn_id.clone(),
        resource_id: request.resource_id.clone(),
        project_root: Arc::new(agent.info.project_root.clone()),
        image_root: Arc::new(agent.info.image_root.clone()),
        package_root: Arc::clone(&package_root),
    };
    let capabilities = LuaDirectCapabilityHandle {
        fs: LuaFileHandle {
            limits: limits.clone(),
        },
        http: LuaHttpHandle {
            client,
            limits: limits.clone(),
        },
        json: LuaJsonHandle,
        log: LuaLogHandle {
            agent_id: context.agent_id.clone(),
            turn_id: context.turn_id.clone(),
            resource_id: context.resource_id.clone(),
        },
        process: LuaProcessHandle {
            limits: limits.clone(),
        },
    };
    Ok((
        package_root,
        LuaExecutionHandle {
            context,
            capabilities,
            limits,
        },
    ))
}

async fn execute_prepared_lua_tool(mut prepared: PreparedLuaToolCall) -> Result<(), LuaTaskError> {
    let result = tokio::time::timeout(
        prepared.handle.limits.max_execution_time,
        execute_lua_tool(&prepared),
    )
    .await
    .unwrap_or_else(|_| {
        Err(ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Lua tool timed out",
        ))
    });
    prepared.response.respond(result);
    Ok(())
}

async fn execute_lua_tool(prepared: &PreparedLuaToolCall) -> Result<String, ToolError> {
    let package = read_lua_tool_package(
        &prepared.package_root,
        &prepared.handle.context.resource_id,
        &prepared.handle.limits,
    )
    .await?;
    let arguments =
        serde_json::from_str::<serde_json::Value>(&prepared.arguments).map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "Lua tool arguments must be valid JSON",
            )
        })?;
    if !arguments.is_object() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "Lua tool arguments must be a JSON object",
        ));
    }
    let validator = jsonschema::validator_for(&package.definition.parameters).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool input schema is invalid",
        )
    })?;
    if !validator.is_valid(&arguments) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "Lua tool arguments do not match input schema",
        ));
    }

    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) };
    lua.set_memory_limit(prepared.handle.limits.max_memory_bytes)
        .map_err(lua_tool_error)?;
    install_execution_hook(&lua, &prepared.handle.limits)?;
    let context = install_lua_environment(&lua, &prepared.handle)?;
    lua.load(&package.script)
        .set_name(prepared.handle.context.resource_id.to_string())
        .exec()
        .map_err(lua_tool_error)?;
    let execute = lua.globals().get::<Function>("execute").map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool main.lua must define execute",
        )
    })?;
    let lua_arguments = lua.to_value(&arguments).map_err(lua_tool_error)?;
    let result = execute
        .call_async::<String>((lua_arguments, context))
        .await
        .map_err(lua_tool_error)?;
    if result.len() > prepared.handle.limits.max_output_bytes {
        return Err(ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Lua tool output exceeds the size limit",
        ));
    }
    Ok(result)
}

fn lua_task_result_system(world: &mut World) {
    for result in world.event_reader::<Result<(), LuaTaskError>>().into_iter() {
        if let Err(error) = result {
            tracing::warn!(error = %error.source, "Lua tool task did not complete");
        }
    }
}

fn find_lua_tool_package(
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    if resource_id.resource_type() != "tool" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "Lua resource must use type tool",
        ));
    }
    let roots = [
        project_root.join(".margatroid").join("tools"),
        image_root.join("tools"),
        home_root.to_path_buf(),
    ];
    for root in roots {
        let package = root
            .join(resource_id.scope())
            .join(resource_id.name())
            .join(resource_id.tag());
        let metadata = match fs::metadata(&package) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "Lua tool package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "Lua tool package is not a directory",
            ));
        }
        for file in [TOOL_METADATA_FILE, TOOL_SCHEMA_FILE, TOOL_SCRIPT_FILE] {
            if !package.join(file).is_file() {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "Lua tool package is incomplete",
                ));
            }
        }
        return Ok(package);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "Lua tool resource was not found",
    ))
}

fn parse_lua_tool_definition(
    metadata_source: &str,
    schema_source: &str,
    resource_id: &ResourceId,
) -> Result<LuaToolDefinition, ToolError> {
    let metadata = toml::from_str::<LuaToolMetadata>(metadata_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool metadata is invalid",
        )
    })?;
    if metadata.schema_version != 1
        || metadata.name.trim().is_empty()
        || metadata.name != resource_id.name()
        || metadata.description.trim().is_empty()
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool metadata does not match the resource",
        ));
    }
    let parameters = serde_json::from_str::<serde_json::Value>(schema_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool input schema is invalid JSON",
        )
    })?;
    if !parameters.is_object() || jsonschema::validator_for(&parameters).is_err() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool input schema is invalid",
        ));
    }
    Ok(LuaToolDefinition {
        metadata,
        parameters,
    })
}

async fn read_lua_tool_package(
    package_root: &Path,
    resource_id: &ResourceId,
    limits: &LuaExecutionLimits,
) -> Result<LuaToolPackage, ToolError> {
    let metadata = read_bounded_async(
        &package_root.join(TOOL_METADATA_FILE),
        limits.max_definition_bytes,
        "Lua tool metadata",
    )
    .await?;
    let schema = read_bounded_async(
        &package_root.join(TOOL_SCHEMA_FILE),
        limits.max_definition_bytes,
        "Lua tool schema",
    )
    .await?;
    let script = read_bounded_async(
        &package_root.join(TOOL_SCRIPT_FILE),
        limits.max_script_bytes,
        "Lua tool script",
    )
    .await?;
    if script.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool script is empty",
        ));
    }
    Ok(LuaToolPackage {
        definition: parse_lua_tool_definition(&metadata, &schema, resource_id)?,
        script,
    })
}

fn install_execution_hook(lua: &Lua, limits: &LuaExecutionLimits) -> Result<(), ToolError> {
    let deadline = Instant::now() + limits.max_execution_time;
    let instructions = Arc::new(AtomicU64::new(0));
    let max_instructions = limits.max_instructions;
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_lua, _debug| {
            let current = instructions.fetch_add(1000, Ordering::Relaxed) + 1000;
            if current > max_instructions || Instant::now() >= deadline {
                return Err(mlua::Error::runtime("Lua execution limit exceeded"));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(lua_tool_error)?;
    Ok(())
}

fn install_lua_environment<'lua>(
    lua: &'lua Lua,
    handle: &LuaExecutionHandle,
) -> Result<Table, ToolError> {
    let context_values = lua.create_table().map_err(lua_tool_error)?;
    context_values
        .set("agent_id", handle.context.agent_id.to_string())
        .map_err(lua_tool_error)?;
    context_values
        .set("turn_id", handle.context.turn_id.clone())
        .map_err(lua_tool_error)?;
    context_values
        .set("resource_id", handle.context.resource_id.to_string())
        .map_err(lua_tool_error)?;
    context_values
        .set(
            "project_root",
            handle.context.project_root.to_string_lossy().as_ref(),
        )
        .map_err(lua_tool_error)?;
    context_values
        .set(
            "image_root",
            handle.context.image_root.to_string_lossy().as_ref(),
        )
        .map_err(lua_tool_error)?;
    context_values
        .set(
            "package_root",
            handle.context.package_root.to_string_lossy().as_ref(),
        )
        .map_err(lua_tool_error)?;
    let context = read_only_proxy(lua, context_values)?;
    let margatroid = lua.create_table().map_err(lua_tool_error)?;
    handle.capabilities.fs.install(lua, &margatroid)?;
    handle.capabilities.http.install(lua, &margatroid)?;
    handle.capabilities.json.install(lua, &margatroid)?;
    handle.capabilities.log.install(lua, &margatroid)?;
    handle.capabilities.process.install(lua, &margatroid)?;
    lua.globals()
        .set("margatroid", margatroid)
        .map_err(lua_tool_error)?;
    Ok(context)
}

fn read_only_proxy(lua: &Lua, values: Table) -> Result<Table, ToolError> {
    let proxy = lua.create_table().map_err(lua_tool_error)?;
    let metatable = lua.create_table().map_err(lua_tool_error)?;
    metatable.set("__index", values).map_err(lua_tool_error)?;
    metatable
        .set(
            "__newindex",
            lua.create_function(|_, (_table, _key, _value): (Table, Value, Value)| {
                Err::<(), _>(mlua::Error::runtime("context is read-only"))
            })
            .map_err(lua_tool_error)?,
        )
        .map_err(lua_tool_error)?;
    metatable
        .set("__metatable", false)
        .map_err(lua_tool_error)?;
    proxy
        .set_metatable(Some(metatable))
        .map_err(lua_tool_error)?;
    Ok(proxy)
}

impl LuaFileHandle {
    fn install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError> {
        let fs_api = lua.create_table().map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        let max_bytes = self.limits.max_output_bytes;
        fs_api
            .set(
                "read_text",
                lua.create_async_function(move |_, path: String| async move {
                    host_timeout(timeout, async {
                        let metadata = tokio::fs::metadata(&path).await?;
                        if metadata.len() > max_bytes as u64 {
                            return Err(std::io::Error::other("file exceeds the size limit"));
                        }
                        tokio::fs::read_to_string(path).await
                    })
                    .await
                    .map_err(mlua::Error::external)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        let max_bytes = self.limits.max_output_bytes;
        fs_api
            .set(
                "write_text",
                lua.create_async_function(move |_, (path, content): (String, String)| async move {
                    if content.len() > max_bytes {
                        return Err(mlua::Error::runtime("content exceeds the size limit"));
                    }
                    host_timeout(timeout, tokio::fs::write(path, content))
                        .await
                        .map_err(mlua::Error::external)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        fs_api
            .set(
                "create_dir_all",
                lua.create_async_function(move |_, path: String| async move {
                    host_timeout(timeout, tokio::fs::create_dir_all(path))
                        .await
                        .map_err(mlua::Error::external)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        fs_api
            .set(
                "remove",
                lua.create_async_function(move |_, path: String| async move {
                    host_timeout(timeout, async {
                        let metadata = tokio::fs::symlink_metadata(&path).await?;
                        if metadata.is_dir() {
                            tokio::fs::remove_dir_all(path).await
                        } else {
                            tokio::fs::remove_file(path).await
                        }
                    })
                    .await
                    .map_err(mlua::Error::external)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        fs_api
            .set(
                "rename",
                lua.create_async_function(move |_, (from, to): (String, String)| async move {
                    host_timeout(timeout, tokio::fs::rename(from, to))
                        .await
                        .map_err(mlua::Error::external)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        let timeout = self.limits.max_host_call_time;
        fs_api
            .set(
                "list",
                lua.create_async_function(move |lua, path: String| async move {
                    let mut entries = host_timeout(timeout, async {
                        let mut directory = tokio::fs::read_dir(path).await?;
                        let mut entries = Vec::new();
                        while let Some(entry) = directory.next_entry().await? {
                            let file_type = entry.file_type().await?;
                            let kind = if file_type.is_file() {
                                "file"
                            } else if file_type.is_dir() {
                                "directory"
                            } else if file_type.is_symlink() {
                                "symlink"
                            } else {
                                "other"
                            };
                            entries.push(LuaDirectoryEntry {
                                name: entry.file_name().to_string_lossy().into_owned(),
                                path: entry.path().to_string_lossy().into_owned(),
                                kind: kind.to_owned(),
                            });
                        }
                        Ok::<_, std::io::Error>(entries)
                    })
                    .await
                    .map_err(mlua::Error::external)?;
                    entries.sort_by(|left, right| left.name.cmp(&right.name));
                    lua.to_value(&entries)
                })
                .map_err(lua_tool_error)?,
            )
            .map_err(lua_tool_error)?;
        margatroid.set("fs", fs_api).map_err(lua_tool_error)
    }
}

#[derive(Serialize)]
struct LuaDirectoryEntry {
    name: String,
    path: String,
    kind: String,
}

impl LuaProcessHandle {
    fn install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError> {
        let api = lua.create_table().map_err(lua_tool_error)?;
        let handle = self.clone();
        api.set(
            "run",
            lua.create_async_function(move |lua, options: Table| {
                let handle = handle.clone();
                async move {
                    let program = options.get::<String>("program")?;
                    let args = options
                        .get::<Option<Vec<String>>>("args")?
                        .unwrap_or_default();
                    let cwd = options.get::<Option<String>>("cwd")?;
                    if program.trim().is_empty()
                        || program.as_bytes().contains(&0)
                        || args.iter().any(|arg| arg.as_bytes().contains(&0))
                        || args.iter().map(String::len).sum::<usize>()
                            > handle.limits.max_argument_bytes
                    {
                        return Err(mlua::Error::runtime("process arguments are invalid"));
                    }
                    let mut command = Command::new(program);
                    command.args(args).kill_on_drop(true);
                    if let Some(cwd) = cwd {
                        command.current_dir(cwd);
                    }
                    command
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    let mut child = command.spawn().map_err(mlua::Error::external)?;
                    let stdout = child.stdout.take().ok_or_else(|| {
                        mlua::Error::runtime("process stdout pipe is unavailable")
                    })?;
                    let stderr = child.stderr.take().ok_or_else(|| {
                        mlua::Error::runtime("process stderr pipe is unavailable")
                    })?;
                    let limit = handle.limits.max_output_bytes;
                    let (status, stdout, stderr) =
                        tokio::time::timeout(handle.limits.max_host_call_time, async {
                            tokio::try_join!(
                                child.wait(),
                                read_process_output(stdout, limit),
                                read_process_output(stderr, limit),
                            )
                        })
                        .await
                        .map_err(|_| mlua::Error::runtime("process timed out"))?
                        .map_err(mlua::Error::external)?;
                    lua.to_value(&LuaProcessOutput {
                        exit_code: status.code(),
                        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
                        stdout_truncated: stdout.truncated,
                        stderr_truncated: stderr.truncated,
                    })
                }
            })
            .map_err(lua_tool_error)?,
        )
        .map_err(lua_tool_error)?;
        margatroid.set("process", api).map_err(lua_tool_error)
    }
}

#[derive(Serialize)]
struct LuaProcessOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

struct ProcessOutputBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_process_output(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<ProcessOutputBuffer> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < count;
    }
    Ok(ProcessOutputBuffer { bytes, truncated })
}

impl LuaHttpHandle {
    fn install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError> {
        let api = lua.create_table().map_err(lua_tool_error)?;
        let handle = self.clone();
        api.set(
            "request",
            lua.create_async_function(move |lua, options: Table| {
                let handle = handle.clone();
                async move {
                    let method = options
                        .get::<Option<String>>("method")?
                        .unwrap_or_else(|| "GET".into());
                    let url = options.get::<String>("url")?;
                    let headers = options
                        .get::<Option<BTreeMap<String, String>>>("headers")?
                        .unwrap_or_default();
                    let body = options.get::<Option<String>>("body")?;
                    let method = reqwest::Method::from_bytes(method.as_bytes())
                        .map_err(mlua::Error::external)?;
                    let mut request = handle.client.request(method, url);
                    for (name, value) in headers {
                        request = request.header(name, value);
                    }
                    if let Some(body) = body {
                        request = request.body(body);
                    }
                    let response =
                        tokio::time::timeout(handle.limits.max_host_call_time, request.send())
                            .await
                            .map_err(|_| mlua::Error::runtime("HTTP request timed out"))?
                            .map_err(mlua::Error::external)?;
                    let status = response.status().as_u16();
                    let headers = response
                        .headers()
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.to_string(),
                                value.to_str().unwrap_or_default().to_owned(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>();
                    let body =
                        read_bounded_response(response, handle.limits.max_output_bytes).await?;
                    lua.to_value(&LuaHttpResponse {
                        status,
                        headers,
                        body,
                    })
                }
            })
            .map_err(lua_tool_error)?,
        )
        .map_err(lua_tool_error)?;
        margatroid.set("http", api).map_err(lua_tool_error)
    }
}

#[derive(Serialize)]
struct LuaHttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

impl LuaJsonHandle {
    fn install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError> {
        let api = lua.create_table().map_err(lua_tool_error)?;
        api.set(
            "encode",
            lua.create_function(|lua, value: Value| {
                let value = lua.from_value::<serde_json::Value>(value)?;
                serde_json::to_string(&value).map_err(mlua::Error::external)
            })
            .map_err(lua_tool_error)?,
        )
        .map_err(lua_tool_error)?;
        api.set(
            "decode",
            lua.create_function(|lua, source: String| {
                let value = serde_json::from_str::<serde_json::Value>(&source)
                    .map_err(mlua::Error::external)?;
                lua.to_value(&value)
            })
            .map_err(lua_tool_error)?,
        )
        .map_err(lua_tool_error)?;
        margatroid.set("json", api).map_err(lua_tool_error)
    }
}

impl LuaLogHandle {
    fn install(&self, lua: &Lua, margatroid: &Table) -> Result<(), ToolError> {
        let api = lua.create_table().map_err(lua_tool_error)?;
        install_log_function(
            lua,
            &api,
            "trace",
            self,
            |agent, turn, resource, message| {
                tracing::trace!(agent, turn_id = turn, resource, "{message}")
            },
        )?;
        install_log_function(
            lua,
            &api,
            "debug",
            self,
            |agent, turn, resource, message| {
                tracing::debug!(agent, turn_id = turn, resource, "{message}")
            },
        )?;
        install_log_function(lua, &api, "info", self, |agent, turn, resource, message| {
            tracing::info!(agent, turn_id = turn, resource, "{message}")
        })?;
        install_log_function(lua, &api, "warn", self, |agent, turn, resource, message| {
            tracing::warn!(agent, turn_id = turn, resource, "{message}")
        })?;
        install_log_function(
            lua,
            &api,
            "error",
            self,
            |agent, turn, resource, message| {
                tracing::error!(agent, turn_id = turn, resource, "{message}")
            },
        )?;
        margatroid.set("log", api).map_err(lua_tool_error)
    }
}

fn install_log_function<F>(
    lua: &Lua,
    api: &Table,
    name: &str,
    handle: &LuaLogHandle,
    log: F,
) -> Result<(), ToolError>
where
    F: Fn(&str, &str, &str, &str) + Send + 'static,
{
    let agent = handle.agent_id.to_string();
    let turn = handle.turn_id.clone();
    let resource = handle.resource_id.to_string();
    api.set(
        name,
        lua.create_function(move |_, message: String| {
            log(&agent, &turn, &resource, &message);
            Ok(())
        })
        .map_err(lua_tool_error)?,
    )
    .map_err(lua_tool_error)
}

async fn read_bounded_response(
    response: reqwest::Response,
    limit: usize,
) -> Result<String, mlua::Error> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(mlua::Error::external)?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(mlua::Error::runtime("HTTP response exceeds the size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(mlua::Error::external)
}

async fn host_timeout<T>(
    timeout: Duration,
    future: impl std::future::Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "host call timed out"))?
}

fn read_bounded_sync(path: &Path, limit: usize, label: &str) -> Result<String, ToolError> {
    let file = fs::File::open(path).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            format!("{label} could not be read"),
        )
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                format!("{label} could not be read"),
            )
        })?;
    decode_bounded(bytes, limit, label)
}

async fn read_bounded_async(path: &Path, limit: usize, label: &str) -> Result<String, ToolError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            format!("{label} could not be read"),
        )
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                format!("{label} could not be read"),
            )
        })?;
    decode_bounded(bytes, limit, label)
}

fn decode_bounded(bytes: Vec<u8>, limit: usize, label: &str) -> Result<String, ToolError> {
    if bytes.len() > limit {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("{label} exceeds the size limit"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("{label} must be UTF-8"),
        )
    })
}

fn lua_tool_error(error: mlua::Error) -> ToolError {
    ToolError::new(
        ToolErrorKind::ExecutionFailed,
        format!("Lua tool execution failed: {error}"),
    )
}

fn normalize_root(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            normalized.push(component.as_os_str());
        }
    }
    Some(normalized)
}
