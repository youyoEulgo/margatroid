use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimeHandle, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Entity, Event, Plugin, Resource, World};
use margatroid_types::ResourceId;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tool_plugin::{
    register_agent_tool, AgentToolEnvironment, ToolCallRequest, ToolCallResponse, ToolError,
    ToolErrorKind, ToolPluginInstalled, ToolTemplate,
};

const SHELL_TYPE: &str = "shell";
const SHELL_FILE: &str = "shell.toml";
const SHELL_SCHEMA_FILE: &str = "input.schema.json";
const SHELL_SCRIPT_FILE: &str = "main.sh";
const SHELL_EXECUTOR_ID: &str = "tool:builtin/shell:latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellExecutionLimits {
    max_definition_bytes: usize,
    max_script_bytes: usize,
    max_argument_bytes: usize,
    max_output_bytes: usize,
    max_execution_time: Duration,
}

impl ShellExecutionLimits {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_definition_bytes: usize,
        max_script_bytes: usize,
        max_argument_bytes: usize,
        max_output_bytes: usize,
        max_execution_time: Duration,
    ) -> Result<Self, ShellError> {
        if [
            max_definition_bytes,
            max_script_bytes,
            max_argument_bytes,
            max_output_bytes,
        ]
        .contains(&0)
            || max_execution_time.is_zero()
        {
            return Err(ShellError::new(
                ShellErrorKind::InvalidLimits,
                "Shell execution limits must be nonzero",
            ));
        }
        Ok(Self {
            max_definition_bytes,
            max_script_bytes,
            max_argument_bytes,
            max_output_bytes,
            max_execution_time,
        })
    }
}

impl Default for ShellExecutionLimits {
    fn default() -> Self {
        Self::new(
            64 * 1024,
            4 * 1024 * 1024,
            1024 * 1024,
            16 * 1024 * 1024,
            Duration::from_secs(15 * 60),
        )
        .expect("default Shell limits are valid")
    }
}

impl Resource for ShellExecutionLimits {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellErrorKind {
    InvalidRoot,
    InvalidLimits,
    AlreadyInstalled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellError {
    kind: ShellErrorKind,
    message: String,
}

impl ShellError {
    fn new(kind: ShellErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ShellErrorKind {
        self.kind
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ShellError {}

pub struct ShellPlugin {
    home_root: Arc<PathBuf>,
    limits: ShellExecutionLimits,
}

impl ShellPlugin {
    pub fn open(home_root: impl Into<PathBuf>) -> Result<Self, ShellError> {
        let home_root = normalize_root(home_root.into()).ok_or_else(|| {
            ShellError::new(
                ShellErrorKind::InvalidRoot,
                "Shell root must be absolute and cannot contain parent traversal",
            )
        })?;
        Ok(Self {
            home_root: Arc::new(home_root),
            limits: ShellExecutionLimits::default(),
        })
    }

    pub fn with_limits(mut self, limits: ShellExecutionLimits) -> Result<Self, ShellError> {
        self.limits = limits;
        Ok(self)
    }
}

impl Plugin for ShellPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>()
            || !app.world().contains_resource::<AsyncRuntimeHandle>()
            || !app.world().contains_resource::<ToolPluginInstalled>()
        {
            panic!("ShellPlugin requires RuntimePlugin, AsyncRuntimePlugin, and ToolPlugin");
        }
        if app.world().contains_resource::<ShellRoots>() {
            panic!("ShellPlugin is already installed");
        }
        app.world_mut().insert_resource(ShellRoots {
            home_root: self.home_root,
        });
        app.world_mut().insert_resource(self.limits);
        app.add_system(RuntimePlugin::UPDATE, shell_register_system)
            .add_system(RuntimePlugin::UPDATE, shell_tool_call_prepare_system)
            .add_async_system(RuntimePlugin::UPDATE, execute_prepared_shell)
            .add_system(RuntimePlugin::UPDATE, shell_task_result_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRegisterRequest {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for ShellRegisterRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRegisterResponse {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub result: Result<(), ToolError>,
}
impl Event for ShellRegisterResponse {}

struct ShellRoots {
    home_root: Arc<PathBuf>,
}
impl Resource for ShellRoots {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellMetadata {
    schema_version: u32,
    name: String,
    description: String,
}

struct ShellDefinition {
    metadata: ShellMetadata,
    parameters: serde_json::Value,
}

struct ShellPackage {
    definition: ShellDefinition,
}

struct ShellCallContext {
    project_root: Arc<PathBuf>,
    resource_id: ResourceId,
}

struct ShellResponseGuard {
    locator: Option<ShellCallLocator>,
    events: RuntimeEventSender,
}

struct ShellCallLocator {
    turn_id: String,
    agent: Entity,
    tool_call_id: String,
}

impl ShellResponseGuard {
    fn new(request: &ToolCallRequest, events: RuntimeEventSender) -> Self {
        Self {
            locator: Some(ShellCallLocator {
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
            .expect("Shell tool response was already sent");
        self.events.send_event(ToolCallResponse {
            turn_id: locator.turn_id,
            agent: locator.agent,
            tool_call_id: locator.tool_call_id,
            result,
        });
    }
}

impl Drop for ShellResponseGuard {
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
                "Shell tool task did not complete",
            )),
        });
    }
}

struct PreparedShellToolCall {
    package_root: Arc<PathBuf>,
    arguments: String,
    context: ShellCallContext,
    limits: ShellExecutionLimits,
    response: ShellResponseGuard,
}
impl Event for PreparedShellToolCall {}

struct ShellTaskError {
    source: AsyncTaskError,
}
impl From<AsyncTaskError> for ShellTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

fn shell_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ShellRegisterRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_shell_resource(world, &request);
        world.send_event(ShellRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            result,
        });
    }
}

fn register_shell_resource(
    world: &mut World,
    request: &ShellRegisterRequest,
) -> Result<(), ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Shell resource registration request is invalid",
        ));
    }
    validate_shell_resource(&request.resource_id)?;
    let environment = world
        .get_component::<AgentToolEnvironment>(request.agent)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ToolEnvironmentMissing,
                "agent tool environment is missing",
            )
        })?;
    let roots = &world
        .get_resource::<ShellRoots>()
        .expect("ShellPlugin is installed")
        .home_root;
    let limits = world
        .get_resource::<ShellExecutionLimits>()
        .expect("ShellPlugin is installed");
    let package_root = find_shell_package(environment, roots, &request.resource_id)?;
    let metadata = read_bounded_sync(
        &package_root.join(SHELL_FILE),
        limits.max_definition_bytes,
        "Shell metadata",
    )?;
    let schema = read_bounded_sync(
        &package_root.join(SHELL_SCHEMA_FILE),
        limits.max_definition_bytes,
        "Shell schema",
    )?;
    let script = read_bounded_sync(
        &package_root.join(SHELL_SCRIPT_FILE),
        limits.max_script_bytes,
        "Shell script",
    )?;
    if script.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell script is empty",
        ));
    }
    let definition = parse_shell_definition(&metadata, &schema, &request.resource_id)?;
    register_agent_tool(
        world,
        request.agent,
        ResourceId::parse(SHELL_EXECUTOR_ID).expect("built-in Shell ID is valid"),
        request.resource_id.clone(),
        ToolTemplate::new(
            request.resource_id.to_string(),
            definition.metadata.description,
            definition.parameters,
        )?,
    )?;
    Ok(())
}

fn shell_tool_call_prepare_system(world: &mut World) {
    let executor_id = ResourceId::parse(SHELL_EXECUTOR_ID).expect("built-in Shell ID is valid");
    let requests = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .filter(|request| request.tool_id == executor_id)
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        match prepare_shell_tool_call(world, &request) {
            Ok((package_root, context, limits)) => {
                let response = ShellResponseGuard::new(&request, world.event_sender());
                world.send_async_event(PreparedShellToolCall {
                    package_root,
                    arguments: request.arguments,
                    context,
                    limits,
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

fn prepare_shell_tool_call(
    world: &World,
    request: &ToolCallRequest,
) -> Result<(Arc<PathBuf>, ShellCallContext, ShellExecutionLimits), ToolError> {
    let limits = world
        .get_resource::<ShellExecutionLimits>()
        .expect("ShellPlugin is installed")
        .clone();
    if request.turn_id.is_empty()
        || request.tool_call_id.is_empty()
        || request.resource_id.resource_type() != SHELL_TYPE
        || request.arguments.len() > limits.max_argument_bytes
        || !world.is_alive(request.agent)
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Shell tool call request is invalid",
        ));
    }
    let environment = world
        .get_component::<AgentToolEnvironment>(request.agent)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ToolEnvironmentMissing,
                "agent tool environment is missing",
            )
        })?;
    let roots = &world
        .get_resource::<ShellRoots>()
        .expect("ShellPlugin is installed")
        .home_root;
    let package_root = Arc::new(find_shell_package(
        environment,
        roots,
        &request.resource_id,
    )?);
    Ok((
        Arc::clone(&package_root),
        ShellCallContext {
            project_root: Arc::new(environment.project_root().to_path_buf()),
            resource_id: request.resource_id.clone(),
        },
        limits,
    ))
}

async fn execute_prepared_shell(mut prepared: PreparedShellToolCall) -> Result<(), ShellTaskError> {
    let result = execute_shell(&prepared).await;
    prepared.response.respond(result);
    Ok(())
}

async fn execute_shell(prepared: &PreparedShellToolCall) -> Result<String, ToolError> {
    let package = read_shell_package(
        &prepared.package_root,
        &prepared.context.resource_id,
        &prepared.limits,
    )
    .await?;
    let arguments =
        serde_json::from_str::<serde_json::Value>(&prepared.arguments).map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "Shell arguments must be valid JSON",
            )
        })?;
    if !arguments.is_object() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "Shell arguments must be a JSON object",
        ));
    }
    let validator = jsonschema::validator_for(&package.definition.parameters).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema is invalid",
        )
    })?;
    if !validator.is_valid(&arguments) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "Shell arguments do not match input schema",
        ));
    }
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "Shell arguments must contain a command string",
            )
        })?;
    let script = prepared.package_root.join(SHELL_SCRIPT_FILE);
    let mut child = Command::new("sh")
        .arg(script)
        .arg(command)
        .current_dir(&*prepared.context.project_root)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell process could not be started",
            )
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell stdout pipe could not be opened",
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell stderr pipe could not be opened",
        )
    })?;
    let limit = prepared.limits.max_output_bytes;
    let result = tokio::time::timeout(prepared.limits.max_execution_time, async {
        tokio::try_join!(
            child.wait(),
            read_bounded(stdout, limit),
            read_bounded(stderr, limit),
        )
    })
    .await
    .map_err(|_| ToolError::new(ToolErrorKind::ExecutionFailed, "Shell process timed out"))?
    .map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process output could not be read",
        )
    })?;
    let (status, stdout, stderr) = result;
    let output = ShellOutput {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    };
    serde_json::to_string(&output).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process result could not be encoded",
        )
    })
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut stream: R,
    limit: usize,
) -> std::io::Result<BoundedOutput> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if read > remaining {
            truncated = true;
        }
    }
    Ok(BoundedOutput { bytes, truncated })
}

#[derive(Serialize)]
struct ShellOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

fn shell_task_result_system(world: &mut World) {
    for result in world
        .event_reader::<Result<(), ShellTaskError>>()
        .into_iter()
    {
        if let Err(error) = result {
            tracing::warn!(error = %error.source, "Shell tool task did not complete");
        }
    }
}

fn find_shell_package(
    environment: &AgentToolEnvironment,
    home_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    validate_shell_resource(resource_id)?;
    let roots = [
        environment
            .project_root()
            .join(".margatroid")
            .join("shells"),
        environment.image_root().join("shells"),
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
                    "Shell package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "Shell package is not a directory",
            ));
        }
        for file in [SHELL_FILE, SHELL_SCHEMA_FILE, SHELL_SCRIPT_FILE] {
            if !package.join(file).is_file() {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "Shell package is incomplete",
                ));
            }
        }
        return Ok(package);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "Shell resource was not found",
    ))
}

fn parse_shell_definition(
    metadata_source: &str,
    schema_source: &str,
    resource_id: &ResourceId,
) -> Result<ShellDefinition, ToolError> {
    let metadata = toml::from_str::<ShellMetadata>(metadata_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell metadata is invalid",
        )
    })?;
    if metadata.schema_version != 1
        || metadata.name.trim().is_empty()
        || metadata.name != resource_id.name()
        || metadata.description.trim().is_empty()
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell metadata does not match the resource",
        ));
    }
    let parameters = serde_json::from_str::<serde_json::Value>(schema_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema is invalid JSON",
        )
    })?;
    if !parameters.is_object() || jsonschema::validator_for(&parameters).is_err() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema is invalid",
        ));
    }
    let command_property = parameters
        .get("properties")
        .and_then(|properties| properties.get("command"))
        .and_then(|command| command.get("type"))
        .and_then(serde_json::Value::as_str);
    let requires_command = parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|value| value == "command"));
    if command_property != Some("string") || !requires_command {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema must require a string command",
        ));
    }
    Ok(ShellDefinition {
        metadata,
        parameters,
    })
}

async fn read_shell_package(
    package_root: &Path,
    resource_id: &ResourceId,
    limits: &ShellExecutionLimits,
) -> Result<ShellPackage, ToolError> {
    let metadata = read_bounded_async(
        &package_root.join(SHELL_FILE),
        limits.max_definition_bytes,
        "Shell metadata",
    )
    .await?;
    let schema = read_bounded_async(
        &package_root.join(SHELL_SCHEMA_FILE),
        limits.max_definition_bytes,
        "Shell schema",
    )
    .await?;
    let script = read_bounded_async(
        &package_root.join(SHELL_SCRIPT_FILE),
        limits.max_script_bytes,
        "Shell script",
    )
    .await?;
    if script.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell script is empty",
        ));
    }
    Ok(ShellPackage {
        definition: parse_shell_definition(&metadata, &schema, resource_id)?,
    })
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
            format!("{label} is not valid UTF-8"),
        )
    })
}

fn validate_shell_resource(resource_id: &ResourceId) -> Result<(), ToolError> {
    if resource_id.resource_type() != SHELL_TYPE {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "Shell resource must use type shell",
        ));
    }
    Ok(())
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
