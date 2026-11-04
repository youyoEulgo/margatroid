use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::{Agent, AgentState};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use futures_util::FutureExt;
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Value as LuaValue, VmState};
use sandbox_resource_core::{read_sandbox, SandboxReadLimits, Sandboxes, SelectedSandbox};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_core::{
    parse_tool_result, InitToolTasks, ToolConfiguration, ToolError, ToolErrorKind as Kind,
    ToolExecuteRequest, ToolExecutionLimits, ToolTaskHandle, ToolTasks,
};
use tool_resource_core::{read_tool, ToolReadLimits, Tools};
use workspace_core::Workspace;

use crate::types::{
    CgroupBoundary, HostFrame, HostWire, NoExternalSchema, RunnerFrame, RunnerRequest, RunnerWire,
    SharedBridge, ToolContext, ToolExecutionContext, ToolLaunchError, ToolLaunchOutput,
    WorkerBridge,
};
use crate::ToolPlugin;

fn error(kind: Kind, message: impl Into<String>) -> ToolError {
    ToolError::new(kind, message)
}

pub(crate) fn handle_init_tool_tasks(world: &mut World, request: InitToolTasks) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let result =
        if !world.is_alive(request.agent) || !world.contains_component::<Agent>(request.agent) {
            Err(error(Kind::AgentMissing, "Agent entity missing"))
        } else {
            if !world.contains_component::<ToolTasks>(request.agent) {
                world.insert_component(request.agent, ToolTasks::new());
            }
            Ok(())
        };
    let _ = reply.send(result);
}

pub(crate) fn handle_tool_execute(world: &mut World, request: ToolExecuteRequest) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let prepared = (|| {
        let agent = world
            .get_component::<Agent>(request.agent)
            .ok_or_else(|| error(Kind::AgentMissing, "Agent entity missing"))?;
        if !matches!(agent.state, AgentState::Idle | AgentState::Working) {
            return Err(error(Kind::Closed, "Agent is not accepting new tool tasks"));
        }
        if request.id.trim().is_empty()
            || request.call_id.trim().is_empty()
            || request.alias.trim().is_empty()
        {
            return Err(error(
                Kind::InvalidRequest,
                "request id, call id and alias must be nonempty",
            ));
        }
        let config = world
            .get_resource::<ToolPlugin>()
            .expect("ToolPlugin configuration missing")
            .clone();
        if serde_json::to_vec(&request.arguments)
            .map_err(|_| error(Kind::InvalidArguments, "arguments are not serializable"))?
            .len()
            > config.limits.argument_bytes
        {
            return Err(error(
                Kind::InvalidArguments,
                "arguments exceed byte budget",
            ));
        }
        let tool_path = world
            .get_component::<Tools>(request.agent)
            .and_then(|tools| tools.get(&request.alias))
            .ok_or_else(|| error(Kind::AliasMissing, "tool alias is not registered"))?
            .to_owned();
        let sandbox_path = world
            .get_component::<SelectedSandbox>(request.agent)
            .and_then(|selected| selected.alias.as_deref())
            .map(|alias| {
                world
                    .get_component::<Sandboxes>(request.agent)
                    .and_then(|sandboxes| sandboxes.get(alias))
                    .map(Path::to_owned)
                    .ok_or_else(|| {
                        error(
                            Kind::AliasMissing,
                            "selected sandbox alias is not registered",
                        )
                    })
            })
            .transpose()?;
        let project_root = world
            .get_component::<Workspace>(agent.workspace)
            .ok_or_else(|| error(Kind::InvalidRequest, "Agent workspace is missing"))?
            .project_root
            .clone();
        let tasks = world
            .get_component::<ToolTasks>(request.agent)
            .ok_or_else(|| error(Kind::TaskMissing, "ToolTasks not initialized"))?
            .clone();
        let (handle, input) = ToolTaskHandle::new(&config.limits)?;
        tasks.register(request.id.clone(), handle.clone())?;
        Ok((
            config,
            tool_path,
            sandbox_path,
            project_root,
            tasks,
            handle,
            input,
        ))
    })();
    let (config, tool_path, sandbox_path, project_root, tasks, handle, input) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = reply.send(Err(error));
            return;
        }
    };
    let schedule = config.schedule.clone();
    let context = ToolExecutionContext {
        request,
        reply: Some(reply),
        tool_path,
        sandbox_path,
        project_root,
        runner_path: config.runner_path,
        limits: config.limits,
        configuration: None,
        handle: handle.clone(),
        tasks,
        input,
    };
    let launch = ToolLaunchGuard {
        fallback: ToolLaunchFallback {
            slot: Arc::new(Mutex::new(Some(context))),
            handle,
        },
        armed: true,
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || launch_tool(launch));
    }));
}

#[derive(Clone)]
pub(crate) struct ToolLaunchFallback {
    slot: Arc<Mutex<Option<ToolExecutionContext>>>,
    handle: ToolTaskHandle,
}
impl ToolLaunchFallback {
    pub(crate) fn fail(&self, failure: ToolError) {
        if let Some(mut context) = self
            .slot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            complete_reply(&mut context, Err(failure.clone()));
            context.handle.complete(Some(failure));
        } else {
            self.handle.set_error(failure);
            self.handle.cancel();
        }
    }
}
struct ToolLaunchGuard {
    fallback: ToolLaunchFallback,
    armed: bool,
}
impl Drop for ToolLaunchGuard {
    fn drop(&mut self) {
        if self.armed {
            self.fallback.fail(error(
                Kind::SubmitFailed,
                "tool launch failed before supervisor handoff",
            ));
        }
    }
}
async fn launch_tool(mut launch: ToolLaunchGuard) -> Result<ToolLaunchOutput, ToolLaunchError> {
    let slot = launch.fallback.slot.clone();
    // Do not let the supervisor claim its context until the registered handle
    // owns the real task, including on a failed attach.
    let (start, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        if started.await.is_err() {
            return;
        }
        let context = slot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(context) = context {
            supervise_tool(context).await;
        }
    });
    launch
        .fallback
        .handle
        .attach_task(task)
        .map_err(|error| ToolLaunchError {
            error,
            fallback: Some(launch.fallback.clone()),
        })?;
    let _ = start.send(());
    launch.armed = false;
    Ok(ToolLaunchOutput)
}

fn complete_reply(
    context: &mut ToolExecutionContext,
    result: Result<tool_core::ToolExecuteResponse, ToolError>,
) {
    if let Some(reply) = context.reply.take() {
        let _ = reply.send(result);
    }
}

async fn supervise_tool(mut context: ToolExecutionContext) {
    let handle = context.handle.clone();
    let result = std::panic::AssertUnwindSafe(async {
        let tool_path = context.tool_path.clone();
        let sandbox_path = context.sandbox_path.clone();
        let script_bytes = context.limits.script_bytes;
        let read = tokio::task::spawn_blocking(move || {
            let tool = read_tool(
                &tool_path,
                &ToolReadLimits {
                    program_bytes: script_bytes,
                    ..ToolReadLimits::default()
                },
            )
            .map_err(|failure| error(Kind::ReadFailed, failure.to_string()))?;
            let policy = sandbox_path
                .map(|path| {
                    read_sandbox(&path, &SandboxReadLimits::default())
                        .map(|input| input.policy)
                        .map_err(|failure| error(Kind::ReadFailed, failure.to_string()))
                })
                .transpose()?;
            Ok::<_, ToolError>((tool, policy))
        });
        let (tool, policy) = read
            .await
            .map_err(|_| error(Kind::TaskPanicked, "tool resource reader failed"))??;
        let validator = jsonschema::options()
            .with_retriever(NoExternalSchema)
            .build(&tool.input_schema)
            .map_err(|_| error(Kind::InvalidDefinition, "tool input schema is invalid"))?;
        if !validator.is_valid(&context.request.arguments) {
            return Err(error(
                Kind::InvalidArguments,
                "arguments do not match current tool schema",
            ));
        }
        if context.handle.cancel_token().is_cancelled() {
            return Err(error(
                Kind::Cancelled,
                "tool task cancelled before runner start",
            ));
        }
        let temporary = tempfile::Builder::new()
            .prefix("margatroid-tool-task-")
            .tempdir()
            .map_err(|_| {
                error(
                    Kind::RunnerStartFailed,
                    "tool temporary directory creation failed",
                )
            })?;
        let request = RunnerRequest {
            protocol_version: 1,
            program: tool.program,
            arguments: context.request.arguments.clone(),
            context: ToolContext {
                task_id: context.request.id.clone(),
                call_id: context.request.call_id.clone(),
                alias: context.request.alias.clone(),
                project_root: context.project_root.clone(),
                temp_dir: temporary.path().to_owned(),
            },
            limits: context.limits.clone(),
        };
        let result = run_runner(&mut context, request, policy).await;
        if let Err(failure) = &result {
            complete_reply(&mut context, Err(failure.clone()));
            context.handle.set_error(failure.clone());
        }
        let temporary_path = temporary.path().to_owned();
        if temporary.close().is_err() {
            loop {
                record_cleanup_failure(
                    &mut context,
                    "tool temporary directory cleanup pending; supervision retained",
                );
                match tokio::fs::remove_dir_all(&temporary_path).await {
                    Ok(()) => break,
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => break,
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        }
        result
    })
    .catch_unwind()
    .await;
    let outcome = result.unwrap_or_else(|_| {
        Err(error(
            Kind::CleanupFailed,
            "tool supervisor panicked before cleanup confirmation",
        ))
    });
    let outcome = match outcome {
        Ok(()) if !handle.has_returned() => {
            Err(error(Kind::MissingReturn, "tool ended without task_return"))
        }
        other => other,
    };
    if let Err(failure) = &outcome {
        complete_reply(&mut context, Err(failure.clone()));
    }
    if outcome
        .as_ref()
        .is_err_and(|failure| failure.kind == Kind::CleanupFailed)
    {
        handle.set_error(outcome.unwrap_err());
    } else {
        handle.complete(outcome.err());
    }
}

impl jsonschema::Retrieve for NoExternalSchema {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema references are forbidden".into())
    }
}

fn wire_limit(limits: &ToolExecutionLimits) -> usize {
    limits
        .result_bytes
        .max(limits.cache_bytes)
        .max(limits.argument_bytes)
        .saturating_mul(6)
        .saturating_add(65536)
}

async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<Option<Vec<u8>>, ToolError> {
    let mut bytes = Vec::new();
    let count = reader
        .take((maximum as u64).saturating_add(1))
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner pipe read failed"))?;
    if count == 0 {
        return Ok(None);
    }
    if count > maximum || bytes.last() != Some(&b'\n') {
        return Err(error(
            Kind::OutputTooLarge,
            "runner frame exceeds limit or lacks delimiter",
        ));
    }
    Ok(Some(bytes))
}

async fn read_json<T: DeserializeOwned, R: AsyncBufRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<Option<T>, ToolError> {
    read_line(reader, maximum)
        .await?
        .map(|line| {
            serde_json::from_slice(&line)
                .map_err(|_| error(Kind::RunnerProtocolFailed, "invalid runner protocol frame"))
        })
        .transpose()
}

async fn write_json<T: Serialize, W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &T,
    maximum: usize,
) -> Result<(), ToolError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| error(Kind::RunnerProtocolFailed, "protocol serialization failed"))?;
    bytes.push(b'\n');
    if bytes.len() > maximum {
        return Err(error(
            Kind::OutputTooLarge,
            "protocol frame exceeds byte budget",
        ));
    }
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner pipe write failed"))?;
    writer
        .flush()
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner pipe flush failed"))
}

fn sandbox_backend() -> Option<PathBuf> {
    std::env::var_os("MARGATROID_SANDBOX_BACKEND")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|path| path.join("landstrip"))
                    .find(|path| path.is_file())
            })
        })
}

fn filtered_environment(command: &mut tokio::process::Command) {
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if ["KEY", "PASSWORD", "SECRET", "TOKEN"]
            .iter()
            .any(|part| upper.contains(part))
            || [
                "LD_PRELOAD",
                "LD_LIBRARY_PATH",
                "DYLD_INSERT_LIBRARIES",
                "LUA_PATH",
                "LUA_CPATH",
                "BASH_ENV",
                "ENV",
                "MARGATROID_RUNNER_CGROUP",
            ]
            .contains(&upper.as_str())
        {
            continue;
        }
        command.env(name, value);
    }
    command
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
}

async fn diagnostic_drain<R: AsyncRead + Unpin>(
    mut pipe: R,
    maximum: usize,
    overflow: CancellationToken,
) -> Result<(), ToolError> {
    let mut total = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe
            .read(&mut chunk)
            .await
            .map_err(|_| error(Kind::RunnerProtocolFailed, "runner diagnostics read failed"))?;
        if count == 0 {
            return Ok(());
        }
        total = total.saturating_add(count);
        if total > maximum {
            overflow.cancel();
        }
    }
}

fn signal_runner(pid: u32) {
    #[cfg(unix)]
    {
        let target = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGTERM);
        let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGCONT);
    }
}

fn record_cleanup_failure(context: &mut ToolExecutionContext, message: &str) {
    let failure = error(Kind::CleanupFailed, message);
    context.handle.set_error(failure.clone());
    complete_reply(context, Err(failure));
}

async fn host_frame(
    context: &mut ToolExecutionContext,
    frame: RunnerFrame,
) -> Result<HostFrame, ToolError> {
    let (request_id, result) = match frame {
        RunnerFrame::Return { request_id, value } => {
            let result = parse_tool_result(&value, &context.limits).and_then(|mut response| {
                context.handle.claim_return()?;
                response.call_id = context.request.call_id.clone();
                response.alias = context.request.alias.clone();
                complete_reply(context, Ok(response));
                Ok(Value::Null)
            });
            (request_id, result)
        }
        RunnerFrame::Write { request_id, value } => (
            request_id,
            context.handle.write(value).map(|()| Value::Null),
        ),
        RunnerFrame::Replace { request_id, value } => (
            request_id,
            context.handle.replace(value).map(|()| Value::Null),
        ),
        RunnerFrame::Read {
            request_id,
            task_id,
        } => (
            request_id,
            context.tasks.read(&task_id).and_then(|value| {
                serde_json::to_value(value).map_err(|_| {
                    error(Kind::RunnerProtocolFailed, "task read serialization failed")
                })
            }),
        ),
        RunnerFrame::Send {
            request_id,
            task_id,
            value,
        } => (
            request_id,
            context.tasks.send(&task_id, value).map(|()| Value::Null),
        ),
        RunnerFrame::Finish {
            request_id,
            task_id,
        } => {
            if task_id == context.request.id {
                return Err(error(
                    Kind::RunnerProtocolFailed,
                    "self finish must remain worker-local",
                ));
            }
            let result = context.tasks.finish(&task_id).await.map(|()| Value::Null);
            (request_id, result)
        }
        _ => {
            return Err(error(
                Kind::RunnerProtocolFailed,
                "invalid worker host request",
            ));
        }
    };
    Ok(HostFrame::Response { request_id, result })
}

async fn run_runner(
    context: &mut ToolExecutionContext,
    request: RunnerRequest,
    policy: Option<Value>,
) -> Result<(), ToolError> {
    let maximum = wire_limit(&request.limits);
    let cancel = context.handle.cancel_token();
    let mut command = tokio::process::Command::new(&context.runner_path);
    command.arg("--margatroid-tool-supervisor");
    if let Some(policy) = policy {
        let backend = sandbox_backend()
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                error(
                    Kind::SandboxUnavailable,
                    "selected sandbox backend unavailable",
                )
            })?;
        let path = request.context.temp_dir.join("policy.json");
        tokio::fs::write(
            &path,
            serde_json::to_vec(&policy)
                .map_err(|_| error(Kind::InvalidDefinition, "invalid sandbox policy"))?,
        )
        .await
        .map_err(|_| error(Kind::RunnerStartFailed, "sandbox staging failed"))?;
        command.arg("--sandbox").arg(backend).arg(path);
    }
    filtered_environment(&mut command);
    command
        .current_dir(&request.context.project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let boundary = CgroupBoundary::create().await?;
    if let Some(boundary) = &boundary {
        command.env("MARGATROID_RUNNER_CGROUP", &boundary.path);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            if let Some(boundary) = boundary {
                boundary.cleanup().await?;
            }
            return Err(error(
                Kind::RunnerStartFailed,
                "tool runner could not start",
            ));
        }
    };
    let pid = child.id().expect("spawned runner has a process id");
    let mut stdin = child.stdin.take().expect("runner stdin piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("runner stdout piped"));
    let stderr = child.stderr.take().expect("runner stderr piped");
    let overflow = CancellationToken::new();
    let diagnostics = tokio::spawn(diagnostic_drain(
        stderr,
        request.limits.diagnostic_bytes,
        overflow.clone(),
    ));
    let (host, mut outgoing) =
        mpsc::channel::<HostFrame>(request.limits.input_capacity.saturating_add(1));
    let writer = tokio::spawn(async move {
        write_json(&mut stdin, &request, maximum).await?;
        while let Some(frame) = outgoing.recv().await {
            write_json(
                &mut stdin,
                &HostWire {
                    protocol_version: 1,
                    frame,
                },
                maximum,
            )
            .await?;
        }
        stdin
            .shutdown()
            .await
            .map_err(|_| error(Kind::RunnerProtocolFailed, "runner stdin shutdown failed"))
    });
    let lifetime = context.handle.clone();
    let mut proven = false;
    let mut result = {
        let exchange = async {
            let mut input_ready = true;
            loop {
                tokio::select! {
                    frame = read_json::<RunnerWire, _>(&mut stdout, maximum) => {
                        let frame = frame?.ok_or_else(|| error(Kind::RunnerProtocolFailed, "runner exited without cleanup proof"))?;
                        if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "runner protocol version mismatch")); }
                        if let RunnerFrame::Configured { configuration } = frame.frame {
                            if context.configuration.is_some() { return Err(error(Kind::RunnerProtocolFailed, "duplicate tool configuration")); }
                            context.handle.configure_lifetime(configuration.timeout_ms)?;
                            context.configuration = Some(configuration);
                            host.send(HostFrame::Configured).await.map_err(|_| error(Kind::RunnerProtocolFailed, "configuration acknowledgement failed"))?;
                            continue;
                        }
                        if matches!(frame.frame, RunnerFrame::InputConsumed) { input_ready = true; continue; }
                        if let RunnerFrame::Stopped { result, cleanup_proven } = frame.frame {
                            if cleanup_proven { proven = true; return result; }
                            if let Err(failure) = result {
                                complete_reply(context, Err(failure.clone()));
                                context.handle.set_error(failure);
                            }
                            record_cleanup_failure(context, "supervisor descendant cleanup incomplete");
                            continue;
                        }
                        if context.configuration.is_none() { return Err(error(Kind::RunnerProtocolFailed, "worker request before configuration")); }
                        let response = host_frame(context, frame.frame).await?;
                        host.send(response).await.map_err(|_| error(Kind::RunnerProtocolFailed, "runner input closed"))?;
                    }
                    value = context.input.recv(), if input_ready && context.configuration.is_some() => {
                        let Some(value) = value else { return Err(error(Kind::InputClosed, "task input channel closed")); };
                        host.send(HostFrame::Input { value }).await.map_err(|_| error(Kind::RunnerProtocolFailed, "runner input closed"))?;
                        input_ready = false;
                    }
                }
            }
        };
        tokio::pin!(exchange);
        tokio::select! {
            biased;
            () = lifetime.lifetime_expired() => {
                lifetime.cancel();
                Err(error(Kind::Timeout, "tool task lifetime exceeded"))
            },
            () = cancel.cancelled() => Err(error(Kind::Cancelled, "tool task cancellation requested")),
            () = overflow.cancelled() => Err(error(Kind::OutputTooLarge, "runner diagnostic budget exceeded")),
            result = &mut exchange => result,
        }
    };
    if let Err(failure) = &result {
        complete_reply(context, Err(failure.clone()));
        context.handle.set_error(failure.clone());
    }
    if !proven {
        signal_runner(pid);
        if result
            .as_ref()
            .is_err_and(|failure| failure.kind != Kind::Cancelled)
        {
            record_cleanup_failure(context, "runner cleanup pending; supervision retained");
        }
        loop {
            let cleanup = async {
                while let Some(frame) = read_json::<RunnerWire, _>(&mut stdout, maximum).await? {
                    if frame.protocol_version == 1 {
                        if let RunnerFrame::Stopped { cleanup_proven, .. } = frame.frame {
                            if cleanup_proven {
                                return Ok::<_, ToolError>(());
                            }
                            record_cleanup_failure(
                                context,
                                "supervisor descendant cleanup incomplete",
                            );
                        }
                    }
                }
                Err(error(
                    Kind::CleanupFailed,
                    "runner did not prove descendant cleanup",
                ))
            };
            match cleanup.await {
                Ok(()) => {
                    proven = true;
                    break;
                }
                _ => {
                    record_cleanup_failure(
                        context,
                        "runner cleanup remains unproven; supervision retained",
                    );
                    signal_runner(pid);
                    if let Some(boundary) = &boundary {
                        if boundary.cleanup().await.is_ok() {
                            proven = true;
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }
    drop(host);
    while child.wait().await.is_err() {
        record_cleanup_failure(context, "runner has not exited; supervision retained");
        signal_runner(pid);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let writer_result = writer
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner input task failed"))?;
    let diagnostic_result = diagnostics
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner diagnostic task failed"))?;
    writer_result?;
    diagnostic_result?;
    if let Some(boundary) = &boundary {
        while boundary.cleanup().await.is_err() {
            record_cleanup_failure(context, "cgroup cleanup incomplete; supervision retained");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    if !proven {
        result = Err(error(Kind::CleanupFailed, "runner cleanup unproven"));
    }
    result
}

impl CgroupBoundary {
    pub(crate) async fn create() -> Result<Option<Self>, ToolError> {
        let Some(root) = std::env::var_os("MARGATROID_TOOL_CGROUP_ROOT") else {
            return Ok(None);
        };
        let root = PathBuf::from(root);
        if !root.is_absolute() || !root.join("cgroup.controllers").is_file() {
            return Err(error(
                Kind::SandboxUnavailable,
                "configured cgroup v2 root unavailable",
            ));
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = root.join(format!(
            "margatroid-tool-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        tokio::fs::create_dir(&path)
            .await
            .map_err(|_| error(Kind::SandboxUnavailable, "cgroup creation failed"))?;
        Ok(Some(Self { path }))
    }
    pub(crate) async fn cleanup(&self) -> Result<(), ToolError> {
        if !self.path.exists() {
            return Ok(());
        }
        tokio::fs::write(self.path.join("cgroup.kill"), "1")
            .await
            .map_err(|_| error(Kind::CleanupFailed, "cgroup kill failed"))?;
        loop {
            let events = tokio::fs::read_to_string(self.path.join("cgroup.events"))
                .await
                .map_err(|_| error(Kind::CleanupFailed, "cgroup status unavailable"))?;
            if events.lines().any(|line| line == "populated 0") {
                return tokio::fs::remove_dir(&self.path)
                    .await
                    .map_err(|_| error(Kind::CleanupFailed, "cgroup removal failed"));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[cfg(target_os = "linux")]
fn descendants() -> std::io::Result<Vec<i32>> {
    let mut parents = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(nix::libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some((_, suffix)) = stat.rsplit_once(") ") else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid proc stat",
            ));
        };
        let ppid = suffix
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc parent")
            })?;
        parents.insert(pid, ppid);
    }
    let root = std::process::id() as i32;
    Ok(parents
        .keys()
        .copied()
        .filter(|pid| {
            let mut current = *pid;
            for _ in 0..parents.len() {
                match parents.get(&current) {
                    Some(parent) if *parent == root => return true,
                    Some(parent) => current = *parent,
                    None => break,
                }
            }
            false
        })
        .collect())
}
#[cfg(target_os = "linux")]
async fn cleanup_descendants() -> bool {
    loop {
        let children = match descendants() {
            Ok(children) => children,
            Err(error) => {
                eprintln!("supervisor descendant scan failed: {error}");
                return false;
            }
        };
        for pid in children {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        loop {
            match nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(-1),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
            ) {
                Err(nix::errno::Errno::ECHILD) => return descendants().is_ok_and(|p| p.is_empty()),
                Ok(nix::sys::wait::WaitStatus::StillAlive) => break,
                Ok(_) => continue,
                Err(nix::errno::Errno::EINTR) => continue,
                Err(_) => return false,
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
async fn shutdown_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {}, _ = hangup.recv() => {} }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn supervisor_main(args: Vec<std::ffi::OsString>) -> i32 {
    let sandbox = match args.as_slice() {
        [] => None,
        [mode] if mode == "--margatroid-tool-supervisor" => None,
        [mode, flag, backend, policy]
            if mode == "--margatroid-tool-supervisor" && flag == "--sandbox" =>
        {
            Some((backend, policy))
        }
        _ => return 2,
    };
    if nix::sys::prctl::set_child_subreaper(true).is_err() {
        return 3;
    }
    if let Some(path) = std::env::var_os("MARGATROID_RUNNER_CGROUP") {
        if tokio::fs::write(
            PathBuf::from(path).join("cgroup.procs"),
            std::process::id().to_string(),
        )
        .await
        .is_err()
        {
            return 3;
        }
    }
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut input = BufReader::new(tokio::io::stdin());
    let request = tokio::select! {
        request = read_json::<RunnerRequest, _>(&mut input, 64 * 1024 * 1024) => request,
        _ = &mut shutdown => return 3,
    };
    let Ok(Some(request)) = request else {
        return 2;
    };
    if request.protocol_version != 1 || request.limits.validate().is_err() {
        return 2;
    }
    let maximum = wire_limit(&request.limits);
    let Ok(executable) = std::env::current_exe() else {
        return 3;
    };
    let mut command = if let Some((backend, policy)) = sandbox {
        let mut command = tokio::process::Command::new(backend);
        command
            .arg("run")
            .arg("-p")
            .arg(policy)
            .arg("--")
            .arg(executable);
        command
    } else {
        tokio::process::Command::new(executable)
    };
    command
        .arg("--margatroid-tool-worker")
        .env_remove("MARGATROID_RUNNER_CGROUP")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut output = tokio::io::stdout();
    let mut configuration: Option<ToolConfiguration> = None;
    let result = match command.spawn() {
        Err(_) => Err(error(
            Kind::RunnerStartFailed,
            "restricted worker could not start",
        )),
        Ok(mut worker) => {
            let mut worker_input = worker.stdin.take().expect("worker stdin piped");
            let mut worker_output =
                BufReader::new(worker.stdout.take().expect("worker stdout piped"));
            let exchange = async {
                write_json(&mut worker_input, &request, maximum).await?;
                let mut acknowledged = false;
                loop {
                    tokio::select! {
                        frame = read_json::<RunnerWire, _>(&mut worker_output, maximum) => {
                            let frame = frame?.ok_or_else(|| error(Kind::ExecutionFailed, "worker exited without terminal frame"))?;
                            if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "worker protocol version mismatch")); }
                            match frame.frame {
                                RunnerFrame::Configured { configuration: value } => {
                                    if configuration.is_some() { return Err(error(Kind::RunnerProtocolFailed, "duplicate worker configuration")); }
                                    value.validate()?;
                                    configuration = Some(value);
                                    write_json(&mut output, &frame, maximum).await?;
                                }
                                RunnerFrame::Done { result } => return result,
                                RunnerFrame::Stopped { .. } => return Err(error(Kind::RunnerProtocolFailed, "worker cannot claim descendant cleanup")),
                                _ if acknowledged => write_json(&mut output, &frame, maximum).await?,
                                _ => return Err(error(Kind::RunnerProtocolFailed, "worker request before configuration acknowledgement")),
                            }
                        }
                        frame = read_json::<HostWire, _>(&mut input, maximum) => {
                            let frame = frame?.ok_or_else(|| error(Kind::Cancelled, "supervisor host input closed"))?;
                            if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "host protocol version mismatch")); }
                            if matches!(frame.frame, HostFrame::Configured) {
                                if acknowledged { return Err(error(Kind::RunnerProtocolFailed, "duplicate configuration acknowledgement")); }
                                configuration.ok_or_else(|| error(Kind::RunnerProtocolFailed, "configuration acknowledgement before configuration"))?;
                                acknowledged = true;
                            } else if !acknowledged { return Err(error(Kind::RunnerProtocolFailed, "host frame before configuration acknowledgement")); }
                            write_json(&mut worker_input, &frame, maximum).await?;
                        }
                    }
                }
            };
            let result = {
                tokio::pin!(exchange);
                tokio::select! {
                    result = &mut exchange => result,
                    _ = &mut shutdown => Err(error(Kind::Cancelled, "supervisor shutdown requested")),
                }
            };
            drop(worker_input);
            while !cleanup_descendants().await {
                let frame = RunnerWire {
                    protocol_version: 1,
                    frame: RunnerFrame::Stopped {
                        result: Err(error(Kind::CleanupFailed, "supervisor cleanup pending")),
                        cleanup_proven: false,
                    },
                };
                let _ = write_json(&mut output, &frame, maximum).await;
            }
            let _ = worker.wait().await;
            let _ = tokio::io::copy(&mut worker_output, &mut tokio::io::sink()).await;
            result
        }
    };
    while !cleanup_descendants().await {
        let frame = RunnerWire {
            protocol_version: 1,
            frame: RunnerFrame::Stopped {
                result: Err(error(Kind::CleanupFailed, "supervisor cleanup pending")),
                cleanup_proven: false,
            },
        };
        let _ = write_json(&mut output, &frame, maximum).await;
    }
    let frame = RunnerWire {
        protocol_version: 1,
        frame: RunnerFrame::Stopped {
            result,
            cleanup_proven: true,
        },
    };
    if write_json(&mut output, &frame, maximum).await.is_ok() {
        0
    } else {
        3
    }
}

#[cfg(not(target_os = "linux"))]
async fn supervisor_main(_: Vec<std::ffi::OsString>) -> i32 {
    3
}

pub async fn runner_main() -> i32 {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args != [std::ffi::OsString::from("--margatroid-tool-worker")] {
        return supervisor_main(args).await;
    }
    let mut input = BufReader::new(tokio::io::stdin());
    let Ok(Some(request)) = read_json::<RunnerRequest, _>(&mut input, 64 * 1024 * 1024).await
    else {
        return 2;
    };
    if request.protocol_version != 1 || request.limits.validate().is_err() {
        return 2;
    }
    let maximum = wire_limit(&request.limits);
    let result = run_tool_worker(request, input).await;
    let frame = RunnerWire {
        protocol_version: 1,
        frame: RunnerFrame::Done { result },
    };
    if write_json(&mut tokio::io::stdout(), &frame, maximum)
        .await
        .is_ok()
    {
        0
    } else {
        3
    }
}

impl WorkerBridge {
    async fn call(&self, frame: impl FnOnce(String) -> RunnerFrame) -> Result<Value, ToolError> {
        let request_id = self.sequence.fetch_add(1, Ordering::Relaxed).to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(request_id.clone(), sender);
        let sent = write_json(
            &mut *self.output.lock().await,
            &RunnerWire {
                protocol_version: 1,
                frame: frame(request_id.clone()),
            },
            self.frame_limit,
        )
        .await;
        let result = match sent {
            Err(error) => Err(error),
            Ok(()) => receiver
                .await
                .map_err(|_| error(Kind::InputClosed, "task host connection closed"))
                .and_then(|result| result),
        };
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&request_id);
        result
    }
}

async fn worker_host_input<R: AsyncBufRead + Unpin>(
    mut reader: R,
    bridge: SharedBridge,
    input: mpsc::Sender<Value>,
) {
    let result = async {
        while let Some(frame) = read_json::<HostWire, _>(&mut reader, bridge.frame_limit).await? {
            if frame.protocol_version != 1 {
                return Err(error(Kind::RunnerProtocolFailed, "host version mismatch"));
            }
            match frame.frame {
                HostFrame::Configured => {
                    return Err(error(
                        Kind::RunnerProtocolFailed,
                        "duplicate configuration acknowledgement",
                    ));
                }
                HostFrame::Input { value } => input
                    .try_send(value)
                    .map_err(|_| error(Kind::RunnerProtocolFailed, "Lua input credit exceeded"))?,
                HostFrame::Response { request_id, result } => {
                    if let Some(reply) = bridge
                        .pending
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(&request_id)
                    {
                        let _ = reply.send(result);
                    }
                }
            }
        }
        Ok::<(), ToolError>(())
    }
    .await;
    let failure = result
        .err()
        .unwrap_or_else(|| error(Kind::InputClosed, "task host connection closed"));
    let pending = std::mem::take(
        &mut *bridge
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
    );
    for (_, sender) in pending {
        let _ = sender.send(Err(failure.clone()));
    }
}

fn lua_failure(failure: mlua::Error) -> ToolError {
    match &failure {
        mlua::Error::ExternalError(source) => source
            .downcast_ref::<ToolError>()
            .cloned()
            .unwrap_or_else(|| error(Kind::ExecutionFailed, failure.to_string())),
        mlua::Error::CallbackError { cause, .. } => lua_failure((**cause).clone()),
        _ => error(Kind::ExecutionFailed, failure.to_string()),
    }
}

async fn run_tool_worker<R: AsyncBufRead + Unpin + Send + 'static>(
    request: RunnerRequest,
    input: R,
) -> Result<(), ToolError> {
    request.limits.validate()?;
    if request.program.len() > request.limits.script_bytes
        || serde_json::to_vec(&request.arguments)
            .map_err(|_| error(Kind::InvalidArguments, "invalid arguments"))?
            .len()
            > request.limits.argument_bytes
    {
        return Err(error(Kind::OutputTooLarge, "runner request exceeds budget"));
    }
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(lua_failure)?;
    lua.set_memory_limit(request.limits.memory_bytes)
        .map_err(lua_failure)?;
    let deadline = Arc::new(Mutex::new(None::<Instant>));
    let hook_deadline = deadline.clone();
    let instructions = AtomicU64::new(0);
    let budget = request.limits.instruction_budget;
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            if hook_deadline
                .lock()
                .unwrap_or_else(|failure| failure.into_inner())
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Err(mlua::Error::external(error(
                    Kind::Timeout,
                    "Lua execution deadline exceeded",
                )));
            }
            if instructions
                .fetch_add(1000, Ordering::Relaxed)
                .saturating_add(1000)
                > budget
            {
                return Err(mlua::Error::external(error(
                    Kind::ExecutionFailed,
                    "Lua instruction budget exceeded",
                )));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(lua_failure)?;
    let globals = lua.globals();
    // Error-catching functions and coroutines could swallow instruction-hook
    // failures and run configuration forever. Install them only after validation.
    let mut execution_globals = Vec::new();
    for name in [
        "pcall",
        "xpcall",
        "load",
        "setmetatable",
        "dofile",
        "loadfile",
        "warn",
    ] {
        execution_globals.push((name, globals.get::<LuaValue>(name).map_err(lua_failure)?));
        globals.set(name, LuaValue::Nil).map_err(lua_failure)?;
    }
    for name in ["io", "os", "print"] {
        globals.set(name, LuaValue::Nil).map_err(lua_failure)?;
    }
    let arguments = lua.to_value(&request.arguments).map_err(lua_failure)?;
    let context = lua.to_value(&request.context).map_err(lua_failure)?;
    lua.load(&request.program)
        .set_name("tool")
        .exec()
        .map_err(|failure| {
            error(
                Kind::InvalidDefinition,
                format!("tool configuration load failed: {failure}"),
            )
        })?;
    let configure = globals.get::<mlua::Function>("configure").map_err(|_| {
        error(
            Kind::InvalidDefinition,
            "tool must define configure(arguments, context)",
        )
    })?;
    let configured: LuaValue = configure
        .call((arguments.clone(), context.clone()))
        .map_err(|failure| {
            error(
                Kind::InvalidDefinition,
                format!("tool configure failed: {failure}"),
            )
        })?;
    let configuration = parse_configuration(&lua, configured)?;
    let mut input = input;
    write_json(
        &mut tokio::io::stdout(),
        &RunnerWire {
            protocol_version: 1,
            frame: RunnerFrame::Configured { configuration },
        },
        wire_limit(&request.limits),
    )
    .await?;
    let acknowledgement = read_json::<HostWire, _>(&mut input, wire_limit(&request.limits)).await?;
    if !matches!(
        acknowledgement,
        Some(HostWire {
            protocol_version: 1,
            frame: HostFrame::Configured
        })
    ) {
        return Err(error(
            Kind::RunnerProtocolFailed,
            "expected tool configuration acknowledgement",
        ));
    }
    let execution_deadline = configuration
        .timeout()
        .map(|timeout| Instant::now() + timeout);
    *deadline
        .lock()
        .unwrap_or_else(|failure| failure.into_inner()) = execution_deadline;
    lua.load_std_libs(StdLib::IO | StdLib::OS | StdLib::COROUTINE)
        .map_err(lua_failure)?;
    for (name, value) in execution_globals {
        globals.set(name, value).map_err(lua_failure)?;
    }
    let (sender, receiver) = mpsc::channel(request.limits.input_capacity);
    let finish = CancellationToken::new();
    let bridge = Arc::new(WorkerBridge {
        output: tokio::sync::Mutex::new(tokio::io::stdout()),
        pending: Mutex::new(BTreeMap::new()),
        input: tokio::sync::Mutex::new(receiver),
        sequence: AtomicU64::new(1),
        frame_limit: wire_limit(&request.limits),
        finish: finish.clone(),
    });
    let host_reader = tokio::spawn(worker_host_input(input, bridge.clone(), sender));
    globals
        .set(
            "print",
            lua.create_function(|_, values: mlua::MultiValue| {
                eprintln!(
                    "{}",
                    values
                        .into_iter()
                        .map(|value| match value {
                            LuaValue::String(value) => value.to_string_lossy(),
                            value => format!("{value:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join("\t")
                );
                Ok(())
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let io: mlua::Table = globals.get("io").map_err(lua_failure)?;
    let diagnostic: LuaValue = io.get("stderr").map_err(lua_failure)?;
    io.get::<mlua::Function>("output")
        .map_err(lua_failure)?
        .call::<()>(diagnostic.clone())
        .map_err(lua_failure)?;
    io.set("stdout", diagnostic).map_err(lua_failure)?;
    io.set(
        "popen",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(error(
                Kind::InvalidRequest,
                "use shell for subprocesses",
            )))
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    let os: mlua::Table = globals.get("os").map_err(lua_failure)?;
    os.set(
        "exit",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(error(
                Kind::InvalidRequest,
                "use task_finish for task exit",
            )))
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    let json = lua.create_table().map_err(lua_failure)?;
    json.set("null", lua.null()).map_err(lua_failure)?;
    json.set(
        "encode",
        lua.create_function(|lua, value: LuaValue| {
            serde_json::to_string(&lua.from_value::<Value>(value)?).map_err(mlua::Error::external)
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    json.set(
        "decode",
        lua.create_function(|lua, value: String| {
            let value: Value = serde_json::from_str(&value).map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    globals.set("json", json.clone()).map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    globals
        .set(
            "task_write",
            lua.create_async_function(move |lua, value: LuaValue| {
                let bridge = task_bridge.clone();
                async move {
                    let value = lua.from_value::<Value>(value)?;
                    bridge
                        .call(|request_id| RunnerFrame::Write { request_id, value })
                        .await
                        .map_err(mlua::Error::external)?;
                    Ok(())
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    globals
        .set(
            "task_replace",
            lua.create_async_function(move |lua, value: LuaValue| {
                let bridge = task_bridge.clone();
                async move {
                    let value = lua.from_value::<Value>(value)?;
                    bridge
                        .call(|request_id| RunnerFrame::Replace { request_id, value })
                        .await
                        .map_err(mlua::Error::external)?;
                    Ok(())
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    let limits = request.limits.clone();
    globals
        .set(
            "task_return",
            lua.create_async_function(move |lua, value: LuaValue| {
                let bridge = task_bridge.clone();
                let limits = limits.clone();
                async move {
                    let value = lua.from_value::<Value>(value)?;
                    parse_tool_result(&value, &limits).map_err(mlua::Error::external)?;
                    bridge
                        .call(|request_id| RunnerFrame::Return { request_id, value })
                        .await
                        .map_err(mlua::Error::external)?;
                    Ok(())
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    globals
        .set(
            "task_read",
            lua.create_async_function(move |lua, task_id: String| {
                let bridge = task_bridge.clone();
                async move {
                    let value = bridge
                        .call(|request_id| RunnerFrame::Read {
                            request_id,
                            task_id,
                        })
                        .await
                        .map_err(mlua::Error::external)?;
                    lua.to_value(&value)
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    globals
        .set(
            "task_send",
            lua.create_async_function(move |lua, (task_id, value): (String, LuaValue)| {
                let bridge = task_bridge.clone();
                async move {
                    let value = lua.from_value::<Value>(value)?;
                    bridge
                        .call(|request_id| RunnerFrame::Send {
                            request_id,
                            task_id,
                            value,
                        })
                        .await
                        .map_err(mlua::Error::external)?;
                    Ok(())
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    let own_id = request.context.task_id.clone();
    globals
        .set(
            "task_finish",
            lua.create_async_function(move |_, task_id: String| {
                let bridge = task_bridge.clone();
                let own_id = own_id.clone();
                async move {
                    if task_id == own_id {
                        bridge.finish.cancel();
                        return std::future::pending::<mlua::Result<()>>().await;
                    }
                    bridge
                        .call(|request_id| RunnerFrame::Finish {
                            request_id,
                            task_id,
                        })
                        .await
                        .map_err(mlua::Error::external)?;
                    Ok(())
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    globals
        .set(
            "task_receive",
            lua.create_async_function(move |lua, ()| {
                let bridge = task_bridge.clone();
                async move {
                    let value = bridge.input.lock().await.recv().await.ok_or_else(|| {
                        mlua::Error::external(error(Kind::InputClosed, "task input closed"))
                    })?;
                    let consumed = bridge.clone();
                    tokio::spawn(async move {
                        let mut output = consumed.output.lock().await;
                        let _ = write_json(
                            &mut *output,
                            &RunnerWire {
                                protocol_version: 1,
                                frame: RunnerFrame::InputConsumed,
                            },
                            consumed.frame_limit,
                        )
                        .await;
                    });
                    lua.to_value(&value)
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    install_shell(&lua, &request)?;
    install_terminal(&lua, &request)?;
    let arguments = lua.to_value(&request.arguments).map_err(lua_failure)?;
    let context = lua.to_value(&request.context).map_err(lua_failure)?;
    globals
        .set("args", arguments.clone())
        .map_err(lua_failure)?;
    globals
        .set("context", context.clone())
        .map_err(lua_failure)?;
    let entry = lua.create_table().map_err(lua_failure)?;
    entry.set("version", 1).map_err(lua_failure)?;
    entry.set("json", json).map_err(lua_failure)?;
    entry.set("context", context.clone()).map_err(lua_failure)?;
    entry
        .set("arguments", arguments.clone())
        .map_err(lua_failure)?;
    for name in [
        "shell",
        "terminal_open",
        "await_any",
        "task_write",
        "task_replace",
        "task_return",
        "task_finish",
        "task_read",
        "task_send",
        "task_receive",
    ] {
        entry
            .set(name, globals.get::<LuaValue>(name).map_err(lua_failure)?)
            .map_err(lua_failure)?;
    }
    let maximum = request.limits.result_bytes;
    entry
        .set(
            "base64_encode",
            lua.create_function(move |_, bytes: mlua::String| {
                use base64::Engine;
                if bytes.as_bytes().len() > maximum {
                    return Err(mlua::Error::external(error(
                        Kind::OutputTooLarge,
                        "base64 input exceeds budget",
                    )));
                }
                Ok(base64::engine::general_purpose::STANDARD.encode(bytes.as_bytes()))
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    lua.set_named_registry_value("margatroid_entry", entry)
        .map_err(lua_failure)?;
    globals
        .set(
            "margatroid",
            lua.create_function(|lua, ()| {
                lua.named_registry_value::<mlua::Table>("margatroid_entry")
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let execution = async {
        if let Some(execute) = globals.get::<Option<mlua::Function>>("execute")? {
            execute.call_async::<LuaValue>((arguments, context)).await?;
        }
        Ok::<(), mlua::Error>(())
    };
    let result = tokio::select! {
        biased;
        () = finish.cancelled() => Ok(()),
        result = execution => result.map_err(lua_failure),
        _ = wait_deadline(execution_deadline.map(tokio::time::Instant::from_std)) => Err(error(Kind::Timeout, "Lua execution deadline exceeded")),
    };
    host_reader.abort();
    let _ = host_reader.await;
    result
}

fn install_shell(lua: &Lua, request: &RunnerRequest) -> Result<(), ToolError> {
    let project_root = request.context.project_root.clone();
    let output_bytes = request.limits.output_bytes;
    let diagnostic_bytes = request.limits.diagnostic_bytes;
    let shell = lua.create_async_function(move |lua, (script, options): (String, Option<mlua::Table>)| {
        let project_root = project_root.clone();
        async move {
            if script.trim().is_empty() { return Err(mlua::Error::external(error(Kind::InvalidArguments, "shell command is empty"))); }
            let requested_limit = options.as_ref().map(|options| options.get::<Option<usize>>("max_output_bytes")).transpose()?.flatten().unwrap_or(output_bytes).min(output_bytes);
            if options.as_ref().map(|options| options.contains_key("timeout_ms")).transpose()?.unwrap_or(false) {
                return Err(mlua::Error::external(error(Kind::InvalidArguments, "shell timeout_ms is unsupported; configure the task lifetime")));
            }
            let use_pty = options.as_ref().map(|options| options.get::<Option<bool>>("pty")).transpose()?.flatten().unwrap_or(false);
            let mut command = tokio::process::Command::new("/bin/sh");
            command.arg("-c").arg(script).current_dir(project_root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            filtered_environment(&mut command);
            #[cfg(target_os = "linux")]
            let terminal = if use_pty { Some(pty_configure(&mut command).map_err(mlua::Error::external)?) } else { None };
            #[cfg(not(target_os = "linux"))]
            if use_pty { return Err(mlua::Error::external(error(Kind::InvalidRequest, "PTY requires Linux"))); }
            let mut child = command.spawn().map_err(mlua::Error::external)?;
            drop(command);
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let work = async {
                let output = async {
                    #[cfg(target_os = "linux")]
                    if let Some(terminal) = terminal {
                        let (stdout, truncated) = pty_drain(terminal, requested_limit).await?;
                        return Ok::<_, mlua::Error>((stdout, String::new(), truncated));
                    }
                    let ((stdout, out_truncated), (stderr, err_truncated)) = tokio::try_join!(capture_pipe(stdout.expect("shell stdout piped"), requested_limit), capture_pipe(stderr.expect("shell stderr piped"), diagnostic_bytes.min(requested_limit)))?;
                    Ok((stdout, stderr, out_truncated || err_truncated))
                };
                let (status, (stdout, stderr, truncated)) = tokio::try_join!(async { child.wait().await.map_err(mlua::Error::external) }, output)?;
                let result = lua.create_table()?;
                result.set("stdout", stdout)?;
                result.set("stderr", stderr)?;
                result.set("code", status.code().unwrap_or(-1))?;
                result.set("exit_code", status.code().unwrap_or(-1))?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    result.set("signal", status.signal())?;
                }
                result.set("truncated", truncated)?;
                result.set("terminal", use_pty)?;
                Ok::<_, mlua::Error>(result)
            };
            let result = work.await;
            if result.is_err() { let _ = child.start_kill(); let _ = child.wait().await; }
            result
        }
    }).map_err(lua_failure)?;
    let globals = lua.globals();
    globals.set("shell", shell.clone()).map_err(lua_failure)?;
    let os: mlua::Table = globals.get("os").map_err(lua_failure)?;
    os.set(
        "execute",
        lua.create_async_function(move |_, command: Option<String>| {
            let shell = shell.clone();
            async move {
                let Some(command) = command else {
                    return Ok((Some(true), None::<String>, None::<i32>));
                };
                let result: mlua::Table = shell.call_async(command).await?;
                if let Some(signal) = result.get::<Option<i32>>("signal")? {
                    return Ok((None, Some("signal".into()), Some(signal)));
                }
                let code = result.get::<i32>("exit_code")?;
                Ok((
                    if code == 0 { Some(true) } else { None },
                    Some("exit".into()),
                    Some(code),
                ))
            }
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    let package = lua.create_table().map_err(lua_failure)?;
    package
        .set(
            "config",
            format!("{}\n;\n?\n!\n-\n", std::path::MAIN_SEPARATOR),
        )
        .map_err(lua_failure)?;
    globals.set("package", package).map_err(lua_failure)?;
    Ok(())
}

async fn capture_pipe<R: AsyncRead + Unpin>(
    mut pipe: R,
    maximum: usize,
) -> mlua::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe.read(&mut chunk).await.map_err(mlua::Error::external)?;
        if count == 0 {
            break;
        }
        let retained = maximum.saturating_sub(bytes.len()).min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < count;
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

#[cfg(target_os = "linux")]
#[allow(deprecated)]
fn pty_configure(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::io::unix::AsyncFd<std::fs::File>> {
    let terminal = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )?;
    let master = std::fs::File::from(terminal.master);
    let slave = std::fs::File::from(terminal.slave);
    let flags = nix::fcntl::fcntl(&master, nix::fcntl::FcntlArg::F_GETFL)?;
    nix::fcntl::fcntl(
        &master,
        nix::fcntl::FcntlArg::F_SETFL(
            nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
        ),
    )?;
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::ioctl(0, nix::libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    tokio::io::unix::AsyncFd::new(master)
}

#[cfg(target_os = "linux")]
async fn pty_drain(
    master: tokio::io::unix::AsyncFd<std::fs::File>,
    limit: usize,
) -> mlua::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let mut ready = master.readable().await.map_err(mlua::Error::external)?;
        match ready.try_io(|fd| {
            let mut file = fd.get_ref();
            std::io::Read::read(&mut file, &mut chunk)
        }) {
            Ok(Ok(0)) => break,
            Ok(Ok(count)) => {
                let retained = limit.saturating_sub(bytes.len()).min(count);
                bytes.extend_from_slice(&chunk[..retained]);
                truncated |= retained < count;
            }
            Ok(Err(error)) if error.raw_os_error() == Some(nix::libc::EIO) => break,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(Err(error)) => return Err(mlua::Error::external(error)),
            Err(_) => continue,
        }
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

fn install_terminal(lua: &mlua::Lua, request: &RunnerRequest) -> Result<(), ToolError> {
    let root = request.context.project_root.clone();
    let output_limit = request.limits.output_bytes.min(request.limits.cache_bytes);
    lua.globals()
        .set(
            "terminal_open",
            lua.create_async_function(move |lua, ()| {
                let root = root.clone();
                async move {
                    #[cfg(target_os = "linux")]
                    {
                        let mut command = tokio::process::Command::new("bash");
                        command
                            .args(["--noprofile", "--norc", "-i"])
                            .current_dir(root)
                            .env("TERM", "xterm-256color")
                            .kill_on_drop(true);
                        let master = pty_configure(&mut command).map_err(mlua::Error::external)?;
                        let child = command.spawn().map_err(mlua::Error::external)?;
                        let terminal = crate::types::TerminalSession {
                            inner: std::sync::Arc::new(crate::types::TerminalInner {
                                master,
                                child: tokio::sync::Mutex::new(child),
                                output: std::sync::Mutex::new(crate::types::TerminalOutput::new(
                                    output_limit,
                                )),
                            }),
                        };
                        lua.create_userdata(terminal)
                    }
                    #[cfg(not(target_os = "linux"))]
                    Err::<mlua::AnyUserData, _>(mlua::Error::external("PTY requires Linux"))
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    lua.globals()
        .set(
            "await_any",
            lua.create_async_function(|_, operations: mlua::Table| async move {
                use futures_util::{stream::FuturesUnordered, StreamExt};
                let mut pending = FuturesUnordered::new();
                for pair in operations.pairs::<String, mlua::Function>() {
                    let (name, function) = pair?;
                    pending.push(async move {
                        function
                            .call_async::<mlua::Value>(())
                            .await
                            .map(|value| (name, value))
                    });
                }
                pending
                    .next()
                    .await
                    .ok_or_else(|| mlua::Error::external("await_any requires an operation"))?
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    Ok(())
}

#[cfg(target_os = "linux")]
impl crate::types::TerminalOutput {
    fn new(limit: usize) -> Self {
        Self {
            // One byte can complete a CSI scroll of up to 24 rows. An extra
            // retained row lets scrollback offset 1 act as a scroll observer.
            parser: vt100::Parser::new(24, 80, 25),
            history: String::new(),
            previous: (String::new(), false),
            limit,
        }
    }

    fn process(&mut self, bytes: &[u8]) -> Result<(String, bool), ToolError> {
        for byte in bytes {
            let screen = self.parser.screen_mut();
            let alternate = screen.alternate_screen();
            screen.set_scrollback(1);
            let before = screen.scrollback();
            self.parser.process(std::slice::from_ref(byte));
            let screen = self.parser.screen_mut();
            if !alternate && !screen.alternate_screen() {
                // With no history the offset is zero and doesn't advance on
                // scroll. Query the clamped maximum to observe the first rows.
                if before == 0 {
                    screen.set_scrollback(usize::MAX);
                }
                let count = screen.scrollback().saturating_sub(before);
                for offset in (1..=count).rev() {
                    screen.set_scrollback(offset);
                    let row = screen.rows(0, 80).next().unwrap_or_default();
                    let separator = if screen.row_wrapped(0) { "" } else { "\n" };
                    if self
                        .history
                        .len()
                        .saturating_add(row.len())
                        .saturating_add(separator.len())
                        > self.limit
                    {
                        return Err(error(
                            Kind::OutputTooLarge,
                            "terminal transcript exceeds output/cache budget",
                        ));
                    }
                    self.history.push_str(&row);
                    self.history.push_str(separator);
                }
            }
            screen.set_scrollback(0);
        }
        let screen = self.parser.screen();
        let alternate = screen.alternate_screen();
        let mut text = if alternate {
            String::new()
        } else {
            self.history.clone()
        };
        text.push_str(&screen.contents());
        if text.len() > self.limit {
            return Err(error(
                Kind::OutputTooLarge,
                "terminal transcript exceeds output/cache budget",
            ));
        }
        let replace =
            alternate || alternate != self.previous.1 || !text.starts_with(&self.previous.0);
        let value = if replace {
            text.clone()
        } else {
            text[self.previous.0.len()..].to_string()
        };
        self.previous = (text, alternate);
        Ok((value, replace))
    }
}

#[cfg(target_os = "linux")]
impl mlua::UserData for crate::types::TerminalSession {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("read", |lua, terminal, ()| async move {
            use mlua::LuaSerdeExt;
            let mut chunk = [0u8; 8192];
            let count = loop {
                let mut ready = tokio::select! {
                    biased;
                    ready = terminal.inner.master.readable() => ready.map_err(mlua::Error::external)?,
                    status = async { terminal.inner.child.lock().await.wait().await } => {
                        let status = status.map_err(mlua::Error::external)?;
                        return lua.to_value(&serde_json::json!({"eof": true, "exit_code": status.code()}));
                    }
                };
                match ready.try_io(|fd| std::io::Read::read(&mut fd.get_ref(), &mut chunk)) {
                    Ok(Ok(count)) => break count,
                    Ok(Err(error)) if error.raw_os_error() == Some(nix::libc::EIO) => break 0,
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Ok(Err(error)) => return Err(mlua::Error::external(error)),
                    Err(_) => continue,
                }
            };
            if count == 0 {
                let status = terminal.inner.child.lock().await.wait().await.map_err(mlua::Error::external)?;
                return lua.to_value(&serde_json::json!({"eof": true, "exit_code": status.code()}));
            }
            let (value, replace) = terminal.inner.output.lock().map_err(|_| mlua::Error::external("terminal output poisoned"))?
                .process(&chunk[..count]).map_err(mlua::Error::external)?;
            lua.to_value(&serde_json::json!({"eof": false, "replace": replace, "text": value}))
        });
        methods.add_async_method("write", |_, terminal, text: mlua::String| async move {
            let bytes = text.as_bytes().to_vec();
            let mut position = 0;
            while position < bytes.len() {
                let mut ready = terminal
                    .inner
                    .master
                    .writable()
                    .await
                    .map_err(mlua::Error::external)?;
                match ready
                    .try_io(|fd| std::io::Write::write(&mut fd.get_ref(), &bytes[position..]))
                {
                    Ok(Ok(0)) => return Err(mlua::Error::external("terminal input closed")),
                    Ok(Ok(count)) => position += count,
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Ok(Err(error)) => return Err(mlua::Error::external(error)),
                    Err(_) => continue,
                }
            }
            Ok(())
        });
        methods.add_async_method("close", |_, terminal, ()| async move {
            let mut child = terminal.inner.child.lock().await;
            child.kill().await.map_err(mlua::Error::external)
        });
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/support/terminal_output.rs"]
mod terminal_output_tests;

async fn wait_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn parse_configuration(lua: &Lua, value: LuaValue) -> Result<ToolConfiguration, ToolError> {
    let LuaValue::Table(table) = value else {
        return Err(error(
            Kind::InvalidDefinition,
            "configure must return a table",
        ));
    };
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(lua_failure)?;
        let LuaValue::String(key) = key else {
            return Err(error(
                Kind::InvalidDefinition,
                "unknown tool configuration field",
            ));
        };
        let key = key.to_str().map_err(lua_failure)?;
        if !matches!(key.as_ref(), "timeout_ms") {
            return Err(error(
                Kind::InvalidDefinition,
                "unknown tool configuration field",
            ));
        }
        if !matches!(value, LuaValue::Integer(value) if value >= 0) {
            return Err(error(
                Kind::InvalidDefinition,
                "configure timeout_ms must be a nonnegative integer",
            ));
        }
    }
    let configuration: ToolConfiguration =
        lua.from_value(LuaValue::Table(table)).map_err(|failure| {
            error(
                Kind::InvalidDefinition,
                format!("invalid tool configuration: {failure}"),
            )
        })?;
    configuration.validate()?;
    Ok(configuration)
}
