use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    candidate_resource_entry, ResourceMapEntry, ToolCallRequest, ToolError, ToolErrorKind,
    ToolRegisterRequest, ToolRegisterResponse, ToolTemplate,
};
use agent_plugin::Agent;
use app_runtime_plugin::{RuntimeEventSender, WorldEventExt};
use async_runtime_plugin::{AsyncTaskError, WorldAsyncExt};
use core_plugin::{Entity, Event, Resource, World};
use margatroid_types::ResourceId;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

const SHELL_TYPE: &str = "shell";
const SHELL_FILE: &str = "shell.toml";
const SHELL_SCHEMA_FILE: &str = "input.schema.json";
const SHELL_SCRIPT_FILE: &str = "main.sh";
const SHELL_EXECUTOR_ID: &str = "tool:builtin/shell:latest";
const SHELL_COMMAND_PROPERTY: &str = "cmd";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellExecutionLimits {
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
    InvalidLimits,
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
}

impl fmt::Display for ShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ShellError {}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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
    script: String,
}

struct ShellCallContext {
    project_root: Arc<PathBuf>,
    resource_id: ResourceId,
    sandbox_policies: Vec<Arc<str>>,
    temp_dir: Arc<PathBuf>,
}

struct ShellResponseGuard {
    locator: Option<ShellCallLocator>,
    events: RuntimeEventSender,
}

struct ShellCallLocator {
    turn_id: String,
    agent: Entity,
    tool_call_id: String,
    resource_id: ResourceId,
}

impl ShellResponseGuard {
    fn new(request: &ToolCallRequest, events: RuntimeEventSender) -> Self {
        Self {
            locator: Some(ShellCallLocator {
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
            .expect("Shell tool response was already sent");
        let failed = result.is_err();
        let content = result.unwrap_or_else(|error| error.to_string());
        self.events.send_event(margatroid_types::AgentMessage {
            id: locator.turn_id,
            agent: locator.agent,
            message: margatroid_types::Message::Tool {
                resource_id: locator.resource_id,
                tool_call_id: locator.tool_call_id,
                content,
                failed,
            },
            usage: None,
        });
    }
}

impl Drop for ShellResponseGuard {
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
                    "Shell tool task did not complete",
                )
                .to_string(),
                failed: true,
            },
            usage: None,
        });
    }
}

pub(crate) struct PreparedShellToolCall {
    package_root: Arc<PathBuf>,
    arguments: String,
    context: ShellCallContext,
    limits: ShellExecutionLimits,
    response: ShellResponseGuard,
    _temp_dir: crate::handler::sandbox::CallTempDir,
}
impl Event for PreparedShellToolCall {}

pub(crate) struct ShellTaskError {
    source: AsyncTaskError,
}
impl From<AsyncTaskError> for ShellTaskError {
    fn from(source: AsyncTaskError) -> Self {
        Self { source }
    }
}

pub(crate) fn shell_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .filter(|request| request.resource_id.resource_type() == "shell")
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_shell_resource(world, &request);
        world.send_event(ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn register_shell_resource(
    world: &mut World,
    request: &ToolRegisterRequest,
) -> Result<ResourceMapEntry, ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Shell resource registration request is invalid",
        ));
    }
    validate_shell_resource(&request.resource_id)?;
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    let limits = world
        .get_resource::<ShellExecutionLimits>()
        .expect("ShellPlugin is installed");
    let package_root = find_shell_package(
        &agent.info.project_root,
        &agent.info.image_root,
        &request.resource_id,
    )?;
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
    candidate_resource_entry(
        request.resource_id.clone(),
        request.alias.clone(),
        ResourceId::parse(SHELL_EXECUTOR_ID).expect("built-in Shell ID is valid"),
        ToolTemplate::new(
            request.resource_id.to_string(),
            definition.metadata.description,
            definition.parameters,
        )?,
    )
}

pub(crate) fn prepare_shell_call(
    world: &mut World,
    request: ToolCallRequest,
) -> Result<(), ToolError> {
    let (package_root, context, limits, temp_dir) = prepare_shell_tool_call(world, &request)?;
    let response = ShellResponseGuard::new(&request, world.event_sender());
    world.send_async_event(PreparedShellToolCall {
        package_root,
        arguments: request.arguments,
        context,
        limits,
        response,
        _temp_dir: temp_dir,
    });
    Ok(())
}

fn prepare_shell_tool_call(
    world: &World,
    request: &ToolCallRequest,
) -> Result<
    (
        Arc<PathBuf>,
        ShellCallContext,
        ShellExecutionLimits,
        crate::handler::sandbox::CallTempDir,
    ),
    ToolError,
> {
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
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    let sandbox_policies = crate::handler::sandbox::active_sandbox_policies(&agent.resources)?;
    let temp_dir = crate::handler::sandbox::CallTempDir::create(&agent.info.project_root)?;
    let package_root = Arc::new(find_shell_package(
        &agent.info.project_root,
        &agent.info.image_root,
        &request.resource_id,
    )?);
    Ok((
        Arc::clone(&package_root),
        ShellCallContext {
            project_root: Arc::new(agent.info.project_root.clone()),
            resource_id: request.resource_id.clone(),
            sandbox_policies,
            temp_dir: Arc::new(temp_dir.path().to_path_buf()),
        },
        limits,
        temp_dir,
    ))
}

pub(crate) async fn execute_prepared_shell(
    mut prepared: PreparedShellToolCall,
) -> Result<(), ShellTaskError> {
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
        .get(SHELL_COMMAND_PROPERTY)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "Shell arguments must contain a cmd string",
            )
        })?;
    let script = prepared.context.temp_dir.join(SHELL_SCRIPT_FILE);
    fs::write(&script, package.script.as_bytes()).map_err(|error| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            format!("Shell script could not be staged: {error}"),
        )
    })?;
    let spawn = crate::handler::sandbox::ConfinedSpawn::new(&prepared.context.sandbox_policies)?;
    let (master, mut child) =
        spawn_pty_shell(&script, command, &prepared.context.project_root, &spawn)?;
    let mut group = crate::handler::sandbox::ProcessGroup::confined(
        spawn.is_confined(),
        child.id().unwrap_or_default(),
    );
    let limit = prepared.limits.max_output_bytes;
    let reader = tokio::task::spawn_blocking(move || read_pty_bounded(master, limit));
    let outcome = tokio::time::timeout(prepared.limits.max_execution_time, async {
        let status = child.wait().await;
        group.reclaim();
        let captured = reader.await;
        (status, captured)
    })
    .await;
    let (status, captured) = match outcome {
        Ok(parts) => parts,
        Err(_) => {
            group.reclaim();
            return Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell process timed out",
            ));
        }
    };
    let status = status.map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process could not be awaited",
        )
    })?;
    let captured = captured
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell output reader could not be joined",
            )
        })?
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell process output could not be read",
            )
        })?;
    let output = ShellOutput {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&captured.bytes).replace("\r\n", "\n"),
        stderr: String::new(),
        stdout_truncated: captured.truncated,
        stderr_truncated: false,
    };
    serde_json::to_string(&output).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process result could not be encoded",
        )
    })
}

#[derive(Default)]
struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn extend_bounded(output: &mut BoundedOutput, chunk: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(output.bytes.len());
    if remaining > 0 {
        output
            .bytes
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    if chunk.len() > remaining {
        output.truncated = true;
    }
}

fn spawn_pty_shell(
    script: &Path,
    command: &str,
    project_root: &Path,
    spawn: &crate::handler::sandbox::ConfinedSpawn,
) -> Result<(std::fs::File, tokio::process::Child), ToolError> {
    let pty = nix::pty::openpty(None, None).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell PTY could not be created",
        )
    })?;
    let master = std::fs::File::from(pty.master);
    let slave_fd = std::os::fd::AsRawFd::as_raw_fd(&pty.slave);
    let duplicate = |error: std::io::Error| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            format!("Shell PTY could not be duplicated: {error}"),
        )
    };
    let stdin = Stdio::from(pty.slave.try_clone().map_err(duplicate)?);
    let stdout = Stdio::from(pty.slave.try_clone().map_err(duplicate)?);
    let stderr = Stdio::from(pty.slave);
    let mut process = spawn.command(Path::new("bash"), project_root)?;
    process
        .arg(script)
        .arg(command)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    unsafe {
        process.pre_exec(move || {
            if nix::unistd::setsid().is_err() {
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::ioctl(slave_fd, nix::libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = process.spawn().map_err(|_| {
        ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "Shell process could not be started",
        )
    })?;
    Ok((master, child))
}

fn read_pty_bounded(mut master: std::fs::File, limit: usize) -> std::io::Result<BoundedOutput> {
    let mut output = BoundedOutput::default();
    let mut buffer = [0_u8; 8192];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => extend_bounded(&mut output, &buffer[..read], limit),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(output)
}

#[derive(Serialize)]
struct ShellOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

pub(crate) fn shell_task_result_system(world: &mut World) {
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
    project_root: &Path,
    image_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    validate_shell_resource(resource_id)?;
    let roots = [
        project_root.join(".margatroid").join("shells"),
        image_root.join("shells"),
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
        .and_then(|properties| properties.get(SHELL_COMMAND_PROPERTY))
        .and_then(|command| command.get("type"))
        .and_then(serde_json::Value::as_str);
    let requires_command = parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|value| value == SHELL_COMMAND_PROPERTY));
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
        script,
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
