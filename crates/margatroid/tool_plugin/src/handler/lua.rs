use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    candidate_resource_entry, ResourceMapEntry, ToolCallRequest, ToolError, ToolErrorKind,
    ToolRegisterRequest, ToolRegisterResponse, ToolTemplate,
};
use agent_plugin::Agent;
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::{Entity, Event, Resource, World};
use margatroid_types::ResourceId;
use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, VmState};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const LUA_RUNTIME_ID: &str = "tool:builtin/lua-runtime:latest";
const TOOL_METADATA_FILE: &str = "tool.toml";
const TOOL_SCHEMA_FILE: &str = "input.schema.json";
const TOOL_SCRIPT_FILE: &str = "main.lua";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaExecutionLimits {
    pub max_definition_bytes: usize,
    pub max_script_bytes: usize,
    pub max_argument_bytes: usize,
    pub max_output_bytes: usize,
    pub max_memory_bytes: usize,
    pub max_instructions: u64,
    pub max_execution_time: Duration,
    pub max_host_call_time: Duration,
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
    InvalidLimits,
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
}

impl fmt::Display for LuaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for LuaError {}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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
    resource_id: ResourceId,
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
                resource_id: request.resource_id.clone(),
            }),
            events,
        }
    }

    fn respond(&mut self, result: Result<String, ToolError>) {
        let locator = self
            .locator
            .take()
            .expect("Lua tool response was already sent");
        let content = result.unwrap_or_else(|error| error.to_string());
        self.events.send_event(margatroid_types::AgentMessage {
            id: locator.turn_id,
            agent: locator.agent,
            message: margatroid_types::Message::Tool {
                resource_id: locator.resource_id,
                tool_call_id: locator.tool_call_id,
                content,
            },
            usage: None,
        });
    }
}

impl Drop for LuaToolResponseGuard {
    fn drop(&mut self) {
        let Some(locator) = self.locator.take() else {
            return;
        };
        self.events.send_event(margatroid_types::AgentMessage {
            id: locator.turn_id,
            agent: locator.agent,
            message: margatroid_types::Message::Tool {
                resource_id: locator.resource_id,
                tool_call_id: locator.tool_call_id,
                content: ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "Lua tool task did not complete",
                )
                .to_string(),
            },
            usage: None,
        });
    }
}

pub(crate) struct PreparedLuaToolCall {
    package_root: Arc<PathBuf>,
    arguments: String,
    handle: LuaExecutionHandle,
    response: LuaToolResponseGuard,
}
impl Event for PreparedLuaToolCall {}

pub(crate) struct LuaTaskError {
    source: AsyncTaskError,
}
impl From<AsyncTaskError> for LuaTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

pub(crate) fn lua_tool_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .filter(|request| request.resource_id.resource_type() == "tool")
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_lua_tool(world, &request);
        world.send_event(ToolRegisterResponse {
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
    request: &ToolRegisterRequest,
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
    let limits = world
        .get_resource::<LuaExecutionLimits>()
        .expect("LuaPlugin is installed");
    let package = find_lua_tool_package(
        &agent.info.project_root,
        &agent.info.image_root,
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

pub(crate) fn prepare_lua_call(
    world: &mut World,
    request: ToolCallRequest,
) -> Result<(), ToolError> {
    let (package_root, handle) = prepare_lua_tool_call(world, &request)?;
    let response = LuaToolResponseGuard::new(&request, world.event_sender());
    world.send_async_event(PreparedLuaToolCall {
        package_root,
        arguments: request.arguments,
        handle,
        response,
    });
    Ok(())
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
    let package_root = Arc::new(find_lua_tool_package(
        &agent.info.project_root,
        &agent.info.image_root,
        &request.resource_id,
    )?);
    let context = LuaCallContext {
        agent_id,
        turn_id: request.turn_id.clone(),
        resource_id: request.resource_id.clone(),
        project_root: Arc::new(agent.info.project_root.clone()),
        image_root: Arc::new(agent.info.image_root.clone()),
        package_root: Arc::clone(&package_root),
    };

    Ok((package_root, LuaExecutionHandle { context, limits }))
}

pub(crate) async fn execute_prepared_lua_tool(
    mut prepared: PreparedLuaToolCall,
) -> Result<(), LuaTaskError> {
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

pub struct LuaToolRunRequest {
    pub package_root: PathBuf,
    pub arguments: String,
    pub agent_id: ResourceId,
    pub turn_id: String,
    pub resource_id: ResourceId,
    pub project_root: PathBuf,
    pub image_root: PathBuf,
    pub limits: LuaExecutionLimits,
}

pub async fn run_lua_tool(request: LuaToolRunRequest) -> Result<String, ToolError> {
    let limits = request.limits.clone();
    let package =
        read_lua_tool_package(&request.package_root, &request.resource_id, &limits).await?;
    let arguments =
        serde_json::from_str::<serde_json::Value>(&request.arguments).map_err(|_| {
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

    let handle = LuaExecutionHandle {
        context: LuaCallContext {
            agent_id: request.agent_id.clone(),
            turn_id: request.turn_id.clone(),
            resource_id: request.resource_id.clone(),
            project_root: Arc::new(request.project_root.clone()),
            image_root: Arc::new(request.image_root.clone()),
            package_root: Arc::new(request.package_root.clone()),
        },
        limits: limits.clone(),
    };
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) };
    lua.set_memory_limit(limits.max_memory_bytes)
        .map_err(lua_tool_error)?;
    install_execution_hook(&lua, &limits)?;
    let context = install_lua_environment(&lua, &handle)?;
    lua.load(&package.script)
        .set_name(request.resource_id.to_string())
        .exec()
        .map_err(lua_tool_error)?;
    let execute = lua.globals().get::<Function>("execute").map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Lua tool main.lua must define execute",
        )
    })?;
    let lua_arguments = lua.to_value(&arguments).map_err(lua_tool_error)?;
    if let Ok(entry) = lua.named_registry_value::<Table>("margatroid_entry") {
        entry
            .set("arguments", &lua_arguments)
            .map_err(lua_tool_error)?;
    }
    let result = execute
        .call_async::<String>((lua_arguments, context))
        .await
        .map_err(lua_tool_error)?;
    if result.len() > limits.max_output_bytes {
        return Err(ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Lua tool output exceeds the size limit",
        ));
    }
    Ok(result)
}

const SANDBOX_POLICY_PATH: &str = "/tmp/margatroid-sandbox-policy.json";

fn sandbox_backend() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MARGATROID_SANDBOX_BACKEND") {
        return Some(PathBuf::from(path));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join("landstrip");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidate = directory.join("landstrip");
    candidate.is_file().then_some(candidate)
}

fn sandbox_environment_ok(backend: &Path) -> Result<(), ToolError> {
    let doctor = std::process::Command::new(backend)
        .arg("doctor")
        .output()
        .map_err(|error| {
            ToolError::new(
                ToolErrorKind::RunnerFailed,
                format!("sandbox backend could not be inspected: {error}"),
            )
        })?;
    let healthy = serde_json::from_slice::<serde_json::Value>(&doctor.stdout)
        .ok()
        .and_then(|value| value.get("ok").and_then(|flag| flag.as_bool()))
        .unwrap_or(false);
    if !doctor.status.success() || !healthy {
        return Err(ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!(
                "sandbox backend is not usable: {}",
                String::from_utf8_lossy(&doctor.stderr).trim()
            ),
        ));
    }
    let validate = std::process::Command::new(backend)
        .args(["policy", "validate", "-p", SANDBOX_POLICY_PATH])
        .output()
        .map_err(|error| {
            ToolError::new(
                ToolErrorKind::RunnerFailed,
                format!("sandbox policy could not be validated: {error}"),
            )
        })?;
    if !validate.status.success() {
        return Err(ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!(
                "sandbox policy is rejected: {}",
                String::from_utf8_lossy(&validate.stderr).trim()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn verify_sandbox() -> Result<(), ToolError> {
    if !Path::new(SANDBOX_POLICY_PATH).is_file() {
        return Ok(());
    }
    let backend = sandbox_backend().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!(
                "sandbox policy {SANDBOX_POLICY_PATH} is present but the landstrip backend was not found; \
                 install landstrip or remove the policy file"
            ),
        )
    })?;
    sandbox_environment_ok(&backend)
}

fn tool_runner_path() -> Result<PathBuf, ToolError> {
    if let Some(path) = std::env::var_os("MARGATROID_TOOL_RUNNER") {
        return Ok(PathBuf::from(path));
    }
    let executable = std::env::current_exe().map_err(|error| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!("tool runner location is unknown: {error}"),
        )
    })?;
    let directory = executable.parent().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            "tool runner location is unknown",
        )
    })?;
    let name = if cfg!(windows) {
        "tool_runner.exe"
    } else {
        "tool_runner"
    };
    Ok(directory.join(name))
}

async fn spawn_tool_runner(request: &LuaToolRunRequest) -> Result<String, ToolError> {
    let payload = serde_json::json!({
        "package_root": request.package_root.to_string_lossy(),
        "arguments": request.arguments,
        "agent_id": request.agent_id.to_string(),
        "turn_id": request.turn_id,
        "resource_id": request.resource_id.to_string(),
        "project_root": request.project_root.to_string_lossy(),
        "image_root": request.image_root.to_string_lossy(),
        "limits": {
            "max_definition_bytes": request.limits.max_definition_bytes,
            "max_script_bytes": request.limits.max_script_bytes,
            "max_argument_bytes": request.limits.max_argument_bytes,
            "max_output_bytes": request.limits.max_output_bytes,
            "max_memory_bytes": request.limits.max_memory_bytes,
            "max_instructions": request.limits.max_instructions,
            "max_execution_time_ms": request.limits.max_execution_time.as_millis() as u64,
            "max_host_call_time_ms": request.limits.max_host_call_time.as_millis() as u64,
        },
    });
    let payload = serde_json::to_vec(&payload).map_err(|error| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!("tool runner request could not be encoded: {error}"),
        )
    })?;
    let runner = tool_runner_path()?;
    let mut command = if Path::new(SANDBOX_POLICY_PATH).is_file() {
        let backend = sandbox_backend().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::RunnerFailed,
                format!("sandbox policy {SANDBOX_POLICY_PATH} is present but the landstrip backend was not found"),
            )
        })?;
        let mut command = Command::new(backend);
        command
            .arg("run")
            .arg("-p")
            .arg(SANDBOX_POLICY_PATH)
            .arg("--")
            .arg(&runner);
        command
    } else {
        Command::new(&runner)
    };
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            format!("tool runner could not start: {error}"),
        )
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let write = async {
            stdin.write_all(&payload).await?;
            stdin.shutdown().await
        };
        write.await.map_err(|error| {
            ToolError::new(
                ToolErrorKind::RunnerFailed,
                format!("tool runner request could not be delivered: {error}"),
            )
        })?;
    }
    let stdout = child.stdout.take().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            "tool runner stdout is unavailable",
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::RunnerFailed,
            "tool runner stderr is unavailable",
        )
    })?;
    let limit = request.limits.max_output_bytes;
    let outcome = tokio::time::timeout(request.limits.max_execution_time, async {
        tokio::try_join!(
            child.wait(),
            read_process_output(stdout, limit),
            read_process_output(stderr, limit),
        )
    })
    .await;
    let (status, stdout, stderr) = match outcome {
        Ok(Ok(parts)) => parts,
        Ok(Err(error)) => {
            return Err(ToolError::new(
                ToolErrorKind::RunnerFailed,
                format!("tool runner stream failed: {error}"),
            ))
        }
        Err(_) => {
            return Err(ToolError::new(
                ToolErrorKind::RunnerFailed,
                "tool runner timed out",
            ))
        }
    };
    let stderr = String::from_utf8_lossy(&stderr.bytes).into_owned();
    if status.success() {
        if stdout.truncated {
            return Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Lua tool output exceeds the size limit",
            ));
        }
        return Ok(String::from_utf8_lossy(&stdout.bytes).into_owned());
    }
    Err(runner_failure(&stderr))
}

fn runner_failure(stderr: &str) -> ToolError {
    let reported = stderr
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .find(|value| {
            matches!(
                value.get("kind").and_then(|kind| kind.as_str()),
                Some("InvalidRequest")
                    | Some("InvalidArguments")
                    | Some("InvalidDefinition")
                    | Some("ExecutionFailed")
                    | Some("RunnerFailed")
            )
        });
    let kind = match reported
        .as_ref()
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
    {
        Some("InvalidRequest") => ToolErrorKind::InvalidRequest,
        Some("InvalidArguments") => ToolErrorKind::InvalidArguments,
        Some("InvalidDefinition") => ToolErrorKind::InvalidDefinition,
        _ => ToolErrorKind::ExecutionFailed,
    };
    let message = reported
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .unwrap_or("Lua tool execution failed")
        .to_owned();
    ToolError::new(kind, message)
}

async fn execute_lua_tool(prepared: &PreparedLuaToolCall) -> Result<String, ToolError> {
    let request = LuaToolRunRequest {
        package_root: prepared.package_root.as_ref().clone(),
        arguments: prepared.arguments.clone(),
        agent_id: prepared.handle.context.agent_id.clone(),
        turn_id: prepared.handle.context.turn_id.clone(),
        resource_id: prepared.handle.context.resource_id.clone(),
        project_root: prepared.handle.context.project_root.as_ref().clone(),
        image_root: prepared.handle.context.image_root.as_ref().clone(),
        limits: prepared.handle.limits.clone(),
    };
    spawn_tool_runner(&request).await
}

pub(crate) fn lua_task_result_system(world: &mut World) {
    for result in world.event_reader::<Result<(), LuaTaskError>>().into_iter() {
        if let Err(error) = result {
            tracing::warn!(error = %error.source, "Lua tool task did not complete");
        }
    }
}

fn find_lua_tool_package(
    project_root: &Path,
    image_root: &Path,
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
    let entry = lua.create_table().map_err(lua_tool_error)?;
    entry.set("version", 1).map_err(lua_tool_error)?;
    entry.set("context", &context).map_err(lua_tool_error)?;
    let json = lua.create_table().map_err(lua_tool_error)?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            let value = lua.from_value::<serde_json::Value>(value)?;
            serde_json::to_string(&value).map_err(mlua::Error::external)
        })
        .map_err(lua_tool_error)?,
    )
    .map_err(lua_tool_error)?;
    json.set(
        "decode",
        lua.create_function(|lua, text: String| {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })
        .map_err(lua_tool_error)?,
    )
    .map_err(lua_tool_error)?;
    entry.set("json", &json).map_err(lua_tool_error)?;
    lua.set_named_registry_value("margatroid_entry", &entry)
        .map_err(lua_tool_error)?;
    let entry_function = lua
        .create_function(|lua, ()| lua.named_registry_value::<Table>("margatroid_entry"))
        .map_err(lua_tool_error)?;
    lua.globals()
        .set("margatroid", entry_function)
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

#[cfg(test)]
mod runner_tests {
    use super::*;

    fn limits(output_bytes: usize, execution_ms: u64) -> LuaExecutionLimits {
        LuaExecutionLimits {
            max_definition_bytes: 1 << 20,
            max_script_bytes: 1 << 20,
            max_argument_bytes: 1 << 20,
            max_output_bytes: output_bytes,
            max_memory_bytes: 1 << 28,
            max_instructions: 1_000_000_000,
            max_execution_time: Duration::from_millis(execution_ms),
            max_host_call_time: Duration::from_millis(execution_ms),
        }
    }

    fn request(limits: LuaExecutionLimits) -> LuaToolRunRequest {
        LuaToolRunRequest {
            package_root: PathBuf::from("/nonexistent/package"),
            arguments: "{}".to_owned(),
            agent_id: ResourceId::parse("agent:test/coder:latest").unwrap(),
            turn_id: "turn-1".to_owned(),
            resource_id: ResourceId::parse("tool:local/glob:latest").unwrap(),
            project_root: PathBuf::from("/tmp"),
            image_root: PathBuf::from("/tmp"),
            limits,
        }
    }

    #[test]
    fn runner_failure_mapping_is_stable() {
        let invalid = runner_failure(r#"{"kind":"InvalidArguments","message":"bad arguments"}"#);
        assert_eq!(invalid.kind(), ToolErrorKind::InvalidArguments);
        assert_eq!(invalid.message(), "bad arguments");

        let failed = runner_failure(r#"{"kind":"ExecutionFailed","message":"boom"}"#);
        assert_eq!(failed.kind(), ToolErrorKind::ExecutionFailed);

        let unreadable = runner_failure("not json at all");
        assert_eq!(unreadable.kind(), ToolErrorKind::ExecutionFailed);
        assert_eq!(unreadable.message(), "Lua tool execution failed");
    }

    #[test]
    fn sandbox_backend_prefers_the_environment_override() {
        std::env::set_var("MARGATROID_SANDBOX_BACKEND", "/opt/landstrip");
        assert_eq!(sandbox_backend(), Some(PathBuf::from("/opt/landstrip")));
        std::env::remove_var("MARGATROID_SANDBOX_BACKEND");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_failures_are_classified() {
        use std::os::unix::fs::PermissionsExt;

        std::env::set_var("MARGATROID_TOOL_RUNNER", "/nonexistent/tool_runner");
        let missing = spawn_tool_runner(&request(limits(1 << 20, 5_000)))
            .await
            .unwrap_err();
        assert_eq!(missing.kind(), ToolErrorKind::RunnerFailed);
        assert!(
            missing.message().contains("could not start"),
            "{}",
            missing.message()
        );

        let script = std::env::temp_dir().join(format!("tool-runner-test-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("MARGATROID_TOOL_RUNNER", &script);
        let timed_out = spawn_tool_runner(&request(limits(1 << 20, 200)))
            .await
            .unwrap_err();
        assert_eq!(timed_out.kind(), ToolErrorKind::RunnerFailed);
        assert!(
            timed_out.message().contains("timed out"),
            "{}",
            timed_out.message()
        );

        std::fs::write(
            &script,
            "#!/bin/sh\ni=0\nwhile [ $i -lt 200 ]; do echo 0123456789; i=$((i+1)); done\n",
        )
        .unwrap();
        let oversized = spawn_tool_runner(&request(limits(16, 5_000)))
            .await
            .unwrap_err();
        assert_eq!(oversized.kind(), ToolErrorKind::ExecutionFailed);
        assert!(
            oversized.message().contains("size limit"),
            "{}",
            oversized.message()
        );

        std::fs::remove_file(&script).ok();
        std::env::remove_var("MARGATROID_TOOL_RUNNER");
    }
}
