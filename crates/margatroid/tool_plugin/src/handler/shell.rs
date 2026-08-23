use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc::SyncSender, Arc, Mutex as StdMutex};
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
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Mutex;

const SHELL_TYPE: &str = "shell";
const SHELL_FILE: &str = "shell.toml";
const SHELL_SCHEMA_FILE: &str = "input.schema.json";
const SHELL_SCRIPT_FILE: &str = "main.sh";
const SHELL_EXECUTOR_ID: &str = "tool:builtin/shell:latest";
static SHELL_MARKER_COUNTER: AtomicU64 = AtomicU64::new(0);

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellRoots {
    pub(crate) home_root: Arc<PathBuf>,
}
impl Resource for ShellRoots {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellMetadata {
    schema_version: u32,
    name: String,
    description: String,
    #[serde(default)]
    persistent: bool,
}

struct ShellDefinition {
    metadata: ShellMetadata,
    parameters: serde_json::Value,
}

struct ShellPackage {
    definition: ShellDefinition,
}

#[derive(Clone, Default)]
pub(crate) struct PersistentShells {
    sessions: Arc<Mutex<HashMap<Entity, Arc<Mutex<PersistentShell>>>>>,
}
impl Resource for PersistentShells {}

struct PersistentShell {
    commands: SyncSender<PtyCommand>,
    child: Arc<StdMutex<std::process::Child>>,
}

struct PtyCommand {
    command: String,
    output_limit: usize,
    response: std::sync::mpsc::SyncSender<Result<ShellOutput, ToolError>>,
}

#[derive(Default)]
struct BoundedOutputBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

impl BoundedOutputBuffer {
    fn append(&mut self, bytes: &[u8], limit: usize) {
        let remaining = limit.saturating_sub(self.bytes.len());
        if remaining > 0 {
            self.bytes
                .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
        if bytes.len() > remaining {
            self.truncated = true;
        }
    }
}

impl PersistentShells {
    async fn execute(
        &self,
        agent: Entity,
        project_root: &Path,
        command: &str,
        output_limit: usize,
        timeout: Duration,
    ) -> Result<ShellOutput, ToolError> {
        let session = {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get(&agent) {
                Arc::clone(session)
            } else {
                let session = Arc::new(Mutex::new(
                    PersistentShell::spawn(project_root, output_limit).await?,
                ));
                sessions.insert(agent, Arc::clone(&session));
                session
            }
        };
        let mut session_guard = session.lock().await;
        match tokio::time::timeout(timeout, session_guard.execute(command, output_limit)).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => {
                drop(session_guard);
                self.reset(agent, &session).await;
                Err(error)
            }
            Err(_) => {
                drop(session_guard);
                self.reset(agent, &session).await;
                Err(ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "Persistent shell command timed out",
                ))
            }
        }
    }

    async fn reset(&self, agent: Entity, expected: &Arc<Mutex<PersistentShell>>) {
        let removed = {
            let mut sessions = self.sessions.lock().await;
            if sessions
                .get(&agent)
                .is_some_and(|session| Arc::ptr_eq(session, expected))
            {
                sessions.remove(&agent)
            } else {
                None
            }
        };
        if let Some(session) = removed {
            let session = session.lock().await;
            if let Ok(mut child) = session.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            };
        }
    }
}

impl PersistentShell {
    async fn spawn(project_root: &Path, _output_limit: usize) -> Result<Self, ToolError> {
        #[cfg(not(unix))]
        {
            let _ = project_root;
            return Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Persistent PTY shells are currently supported on Unix only",
            ));
        }
        #[cfg(unix)]
        {
            use nix::pty::openpty;
            use std::fs::File;
            use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};

            let pair = openpty(None, None).map_err(|_| {
                ToolError::new(ToolErrorKind::ExecutionFailed, "PTY could not be allocated")
            })?;
            let master = unsafe { File::from_raw_fd(pair.master.into_raw_fd()) };
            let slave = unsafe { File::from_raw_fd(pair.slave.into_raw_fd()) };
            let slave_fd = slave.as_raw_fd();
            let stdin = slave.try_clone().map_err(|_| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "PTY stdin could not be cloned",
                )
            })?;
            let stdout = slave.try_clone().map_err(|_| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "PTY stdout could not be cloned",
                )
            })?;
            let stderr = slave.try_clone().map_err(|_| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "PTY stderr could not be cloned",
                )
            })?;
            let mut command = std::process::Command::new("bash");
            command
                .args(["--noprofile", "--norc", "-i"])
                .current_dir(project_root)
                .env("PS1", "")
                .stdin(std::process::Stdio::from(stdin))
                .stdout(std::process::Stdio::from(stdout))
                .stderr(std::process::Stdio::from(stderr));
            unsafe {
                command.pre_exec(move || {
                    if nix::unistd::setsid().is_err() {
                        return Err(std::io::Error::last_os_error());
                    }
                    if nix::libc::ioctl(slave_fd, nix::libc::TIOCSCTTY, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().map_err(|_| {
                ToolError::new(
                    ToolErrorKind::ExecutionFailed,
                    "Persistent PTY Bash could not start",
                )
            })?;
            drop(slave);
            let child = Arc::new(StdMutex::new(child));
            let (commands, receiver) = std::sync::mpsc::sync_channel::<PtyCommand>(1);
            let worker_child = Arc::clone(&child);
            std::thread::Builder::new()
                .name("margatroid-persistent-pty".into())
                .spawn(move || pty_worker(master, receiver, worker_child))
                .map_err(|_| {
                    ToolError::new(ToolErrorKind::ExecutionFailed, "PTY worker could not start")
                })?;
            Ok(Self { commands, child })
        }
    }

    async fn execute(
        &mut self,
        command: &str,
        output_limit: usize,
    ) -> Result<ShellOutput, ToolError> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        self.commands
            .send(PtyCommand {
                command: command.into(),
                output_limit,
                response,
            })
            .map_err(|_| {
                ToolError::new(ToolErrorKind::ExecutionFailed, "PTY worker is unavailable")
            })?;
        tokio::task::spawn_blocking(move || receiver.recv())
            .await
            .map_err(|_| ToolError::new(ToolErrorKind::ExecutionFailed, "PTY worker panicked"))?
            .map_err(|_| ToolError::new(ToolErrorKind::ExecutionFailed, "PTY worker stopped"))?
    }
}

struct PersistentCapture {
    stdout: String,
    exit_code: Option<i32>,
    stdout_truncated: bool,
}

fn pty_worker(
    mut master: std::fs::File,
    receiver: std::sync::mpsc::Receiver<PtyCommand>,
    _child: Arc<StdMutex<std::process::Child>>,
) {
    for request in receiver {
        let nonce = format!("{:016x}", rand_nonce());
        let start = format!("__MARGATROID_SHELL_START_{nonce}__");
        let end = format!("__MARGATROID_SHELL_END_{nonce}__:");
        let wrapped = format!(
            "stty -echo; printf '%s\\n' {}; eval -- {}; status=$?; printf '%s%s\\n' {} \"$status\"",
            quote_bash(&start),
            quote_bash(&request.command),
            quote_bash(&end),
        );
        let result = master
            .write_all(wrapped.as_bytes())
            .and_then(|_| master.write_all(b"\n"))
            .and_then(|_| master.flush())
            .map_err(|_| ToolError::new(ToolErrorKind::ExecutionFailed, "PTY input write failed"))
            .and_then(|_| {
                read_until_shell_marker_sync(&mut master, &start, &end, request.output_limit)
            })
            .map(|capture| ShellOutput {
                exit_code: capture.exit_code,
                stdout: capture.stdout,
                stderr: String::new(),
                stdout_truncated: capture.stdout_truncated,
                stderr_truncated: false,
            });
        let failed = result.is_err();
        let _ = request.response.send(result);
        if failed {
            break;
        }
    }
}

fn read_until_shell_marker_sync(
    stdout: &mut std::fs::File,
    start: &str,
    end: &str,
    limit: usize,
) -> Result<PersistentCapture, ToolError> {
    let mut pending = Vec::new();
    let mut captured = BoundedOutputBuffer::default();
    let end_bytes = end.as_bytes();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stdout.read(&mut buffer).map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Persistent shell stdout read failed",
            )
        })?;
        if read == 0 {
            return Err(ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Persistent shell exited unexpectedly",
            ));
        }
        pending.extend_from_slice(&buffer[..read]);
        if let Some((position, status)) = find_end_marker(&pending, end_bytes) {
            captured.append(&pending[..position], limit);
            let mut output = String::from_utf8_lossy(&captured.bytes).into_owned();
            if let Some(start_position) = output.find(start) {
                output = output[start_position + start.len()..]
                    .trim_start_matches(['\r', '\n'])
                    .to_string();
            }
            return Ok(PersistentCapture {
                stdout: output.replace("\r\n", "\n"),
                exit_code: Some(status),
                stdout_truncated: captured.truncated,
            });
        }
        let retain = end_bytes.len().saturating_sub(1);
        if pending.len() > retain {
            let flush = pending.len() - retain;
            captured.append(&pending[..flush], limit);
            pending.drain(..flush);
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_end_marker(haystack: &[u8], marker: &[u8]) -> Option<(usize, i32)> {
    let mut offset = 0;
    while let Some(relative) = find_bytes(&haystack[offset..], marker) {
        let position = offset + relative;
        let status_start = position + marker.len();
        let newline = haystack[status_start..]
            .iter()
            .position(|byte| *byte == b'\n')?;
        let status = std::str::from_utf8(&haystack[status_start..status_start + newline])
            .ok()?
            .trim()
            .parse::<i32>();
        if let Ok(status) = status {
            return Some((position, status));
        }
        offset = position + marker.len();
    }
    None
}

fn quote_bash(value: &str) -> String {
    format!(
        "$'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
    )
}

fn rand_nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_nanos() as u64
        ^ (std::process::id() as u64)
        ^ SHELL_MARKER_COUNTER.fetch_add(1, Ordering::Relaxed)
}

struct ShellCallContext {
    agent: Entity,
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
    persistent_shells: Option<PersistentShells>,
    response: ShellResponseGuard,
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
    let roots = &world
        .get_resource::<ShellRoots>()
        .expect("ShellPlugin is installed")
        .home_root;
    let limits = world
        .get_resource::<ShellExecutionLimits>()
        .expect("ShellPlugin is installed");
    let package_root = find_shell_package(
        &agent.info.project_root,
        &agent.info.image_root,
        roots,
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
    let (package_root, context, limits, persistent_shells) =
        prepare_shell_tool_call(world, &request)?;
    let response = ShellResponseGuard::new(&request, world.event_sender());
    world.send_async_event(PreparedShellToolCall {
        package_root,
        arguments: request.arguments,
        context,
        limits,
        persistent_shells,
        response,
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
        Option<PersistentShells>,
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
    let roots = &world
        .get_resource::<ShellRoots>()
        .expect("ShellPlugin is installed")
        .home_root;
    let package_root = Arc::new(find_shell_package(
        &agent.info.project_root,
        &agent.info.image_root,
        roots,
        &request.resource_id,
    )?);
    Ok((
        Arc::clone(&package_root),
        ShellCallContext {
            agent: request.agent,
            project_root: Arc::new(agent.info.project_root.clone()),
            resource_id: request.resource_id.clone(),
        },
        limits,
        world.get_resource::<PersistentShells>().cloned(),
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
        .get("command")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "Shell arguments must contain a command string",
            )
        })?;
    if package.definition.metadata.persistent {
        let shells = prepared.persistent_shells.as_ref().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Persistent shell manager is unavailable",
            )
        })?;
        let output = shells
            .execute(
                prepared.context.agent,
                &prepared.context.project_root,
                command,
                prepared.limits.max_output_bytes,
                prepared.limits.max_execution_time,
            )
            .await?;
        return serde_json::to_string(&output).map_err(|_| {
            ToolError::new(
                ToolErrorKind::ExecutionFailed,
                "Shell process result could not be encoded",
            )
        });
    }
    let script = prepared.package_root.join(SHELL_SCRIPT_FILE);
    let mut child = Command::new("bash")
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
    home_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    validate_shell_resource(resource_id)?;
    let roots = [
        project_root.join(".margatroid").join("shells"),
        image_root.join("shells"),
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
