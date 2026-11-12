pub(crate) mod memory;
use memory::MemoryBoundary;
mod diagnostics;
use diagnostics::*;
use lua_runtime_core::host::{self, data, process};
use storage_core::{AgentLogLevel, AgentLogRecord, AgentLogSource, AgentStorage};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::{Agent, AgentState};
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::WorldClosureSender;
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
    ToolExecuteRequest, ToolTaskHandle, ToolTasks,
};
use tool_resource_core::{InjectedToolDefinition, ToolEntry, Tools};
use workspace_core::Workspace;

use crate::types::{
    HostFrame, HostWire, NoExternalSchema, RunnerFrame, RunnerRequest, RunnerWire, SharedBridge,
    ToolContext, ToolExecutionContext, ToolLaunchError, ToolLaunchOutput, WorkerBridge,
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
    let accepted = world
        .get_component::<Agent>(request.agent)
        .is_some_and(|agent| {
            world.is_alive(request.agent)
                && agent.state == AgentState::Working
                && !request.turn.is_empty()
                && agent.turn.as_deref() == Some(request.turn.as_str())
        });
    if !accepted {
        let _ = reply.send(Ok(tool_core::ToolExecuteResponse::aborted(
            request.call_id.clone(),
            request.alias.clone(),
        )));
        return;
    }
    let prepared = (|| {
        let agent = world
            .get_component::<Agent>(request.agent)
            .ok_or_else(|| error(Kind::AgentMissing, "Agent entity missing"))?;
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
        let tool_entry = world
            .get_component::<Tools>(request.agent)
            .and_then(|tools| tools.get(&request.alias))
            .ok_or_else(|| error(Kind::AliasMissing, "tool alias is not registered"))?
            .clone();
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
        let storage = world
            .get_component::<AgentStorage>(request.agent)
            .ok_or_else(|| {
                error(
                    Kind::InvalidRequest,
                    "AgentStorage is required for tool execution",
                )
            })?
            .clone();
        let agent_name = agent.name.clone();
        validate_arguments(&tool_entry.input_schema, &request.arguments)?;
        Ok((
            config,
            tool_entry,
            sandbox_path,
            project_root,
            tasks,
            storage,
            agent_name,
        ))
    })();
    let (config, tool_entry, sandbox_path, project_root, tasks, storage, agent_name) =
        match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
    let Ok((handle, input)) = ToolTaskHandle::new(
        &config.limits,
        request.call_id.clone(),
        request.alias.clone(),
        reply,
    ) else {
        // Handle construction delivers its validation failure to the original reply.
        return;
    };
    if let Err(failure) = tasks.register(request.id.clone(), handle.clone()) {
        let _ = handle.return_result(Err(failure));
        return;
    }
    let schedule = config.schedule.clone();
    let context = ToolExecutionContext {
        storage,
        agent_name,
        diagnostic_received: false,
        request,
        tool_entry,
        world: Arc::downgrade(
            &world
                .get_resource::<crate::ToolRuntime>()
                .expect("Tool runtime missing")
                .0,
        ),
        sandbox_path,
        project_root,
        runner_path: config.runner_path,
        limits: config.limits,
        configuration: None,
        memory_pool: world
            .get_resource::<crate::ToolRuntime>()
            .expect("Tool runtime missing")
            .1
            .clone(),
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
            context
                .tasks
                .complete_cleanup(&context.request.id, &context.handle, Some(failure));
        } else {
            let _ = self.handle.return_result(Err(failure.clone()));
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
    let _ = context.handle.return_result(result);
}

async fn supervise_tool(mut context: ToolExecutionContext) {
    let handle = context.handle.clone();
    let result = std::panic::AssertUnwindSafe(async {
        let tool_path = context.tool_entry.path.clone();
        let sandbox_path = context.sandbox_path.clone();
        let read = tokio::task::spawn_blocking(move || {
            let program = read_program(&tool_path)?;
            let policy = sandbox_path
                .map(|path| {
                    read_sandbox(&path, &SandboxReadLimits::default())
                        .map(|input| input.policy)
                        .map_err(|failure| error(Kind::ReadFailed, failure.to_string()))
                })
                .transpose()?;
            Ok::<_, ToolError>((program, policy))
        });
        let (program, policy) = read
            .await
            .map_err(|_| error(Kind::TaskPanicked, "tool resource reader failed"))??;
        validate_arguments(&context.tool_entry.input_schema, &context.request.arguments)?;
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
            program,
            arguments: context.request.arguments.clone(),
            context: ToolContext {
                agent_name: context.agent_name.clone(),
                turn: context.request.turn.clone(),
                source_origin: resource_dir(&context.tool_entry.path)?
                    .join("main.lua")
                    .to_string_lossy()
                    .into_owned(),
                task_id: context.request.id.clone(),
                call_id: context.request.call_id.clone(),
                alias: context.request.alias.clone(),
                owner_task_id: context.tool_entry.owner_task_id.clone(),
                binding: context.tool_entry.binding.clone(),
                resource_dir: resource_dir(&context.tool_entry.path)?.to_owned(),
                project_root: context.project_root.clone(),
                temp_dir: temporary.path().to_owned(),
            },
            limits: context.limits.clone(),
        };
        let result = run_runner(&mut context, request, policy).await;
        if let Err(failure) = &result {
            report_host_failure(&mut context, failure).await;
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
    // A normal return from the supervised future confirms all cleanup paths,
    // even when the original execution error itself was CleanupFailed.
    let cleanup_confirmed = result.is_ok();
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
        report_host_failure(&mut context, failure).await;
        complete_reply(&mut context, Err(failure.clone()));
    }
    if let Err(failure) = cleanup_owner(&context).await {
        handle.set_error(failure.clone());
        complete_reply(&mut context, Err(failure));
        return;
    }
    if cleanup_confirmed {
        context
            .tasks
            .complete_cleanup(&context.request.id, &handle, outcome.err());
    } else {
        handle.set_error(outcome.unwrap_err());
    }
}

fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::options()
        .with_retriever(NoExternalSchema)
        .build(schema)
        .map_err(|_| error(Kind::InvalidDefinition, "tool input schema is invalid"))?;
    if !validator.is_valid(arguments) {
        return Err(error(
            Kind::InvalidArguments,
            "arguments do not match tool schema",
        ));
    }
    Ok(())
}

fn resource_dir(path: &Path) -> Result<&Path, ToolError> {
    path.parent()
        .ok_or_else(|| error(Kind::ReadFailed, "tool manifest has no resource directory"))
}

fn read_program(path: &Path) -> Result<String, ToolError> {
    use std::io::Read;
    let mut file = std::fs::File::open(resource_dir(path)?.join("main.lua"))
        .map_err(|failure| error(Kind::ReadFailed, failure.to_string()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|failure| error(Kind::ReadFailed, failure.to_string()))?;
    String::from_utf8(bytes).map_err(|_| error(Kind::ReadFailed, "tool program is not UTF-8"))
}

async fn in_world<T: Send + 'static>(
    sender: &std::sync::Weak<WorldClosureSender>,
    work: impl FnOnce(&mut World) -> Result<T, ToolError> + Send + 'static,
) -> Result<T, ToolError> {
    let sender = sender
        .upgrade()
        .ok_or_else(|| error(Kind::CleanupFailed, "tool World is no longer available"))?;
    let (reply, receiver) = oneshot::channel();
    sender.send(move |world| {
        let _ = reply.send(work(world));
    });
    drop(sender);
    receiver
        .await
        .map_err(|_| error(Kind::CleanupFailed, "tool World action reply lost"))?
}

async fn inject_tools(
    context: &ToolExecutionContext,
    definitions: Vec<InjectedToolDefinition>,
) -> Result<(), ToolError> {
    if context.configuration.is_none() {
        return Err(error(
            Kind::InvalidRequest,
            "tool configuration not confirmed",
        ));
    }
    let agent = context.request.agent;
    let task_id = context.request.id.clone();
    let path = context.tool_entry.path.clone();
    in_world(&context.world, move |world| {
        let tasks = world
            .get_component::<ToolTasks>(agent)
            .ok_or_else(|| error(Kind::TaskMissing, "Agent tool tasks missing"))?;
        let handle = tasks.get(&task_id)?;
        if handle.cancel_token().is_cancelled() || handle.is_cleanup_complete() {
            return Err(error(
                Kind::Cancelled,
                "tool task cancelled before injection",
            ));
        }
        for definition in &definitions {
            if let Some(owner) = &definition.owner_task_id {
                let owner = tasks.get(owner)?;
                if owner.cancel_token().is_cancelled() || owner.is_cleanup_complete() {
                    return Err(error(
                        Kind::InvalidRequest,
                        "tool owner is no longer available for injection",
                    ));
                }
            }
        }
        let entries = definitions
            .into_iter()
            .map(|definition| {
                (
                    definition.alias,
                    ToolEntry {
                        path: path.clone(),
                        description: definition.description,
                        input_schema: definition.input_schema,
                        owner_task_id: definition.owner_task_id,
                        binding: definition.binding,
                    },
                )
            })
            .collect();
        world
            .get_component_mut::<Tools>(agent)
            .ok_or_else(|| error(Kind::AgentMissing, "Agent tools missing"))?
            .insert_batch(entries)
            .map_err(|failure| error(Kind::InvalidDefinition, failure.to_string()))
    })
    .await
}

async fn cleanup_owner(context: &ToolExecutionContext) -> Result<(), ToolError> {
    context.handle.cancel();
    let agent = context.request.agent;
    let task_id = context.request.id.clone();
    in_world(&context.world, move |world| {
        if let Some(tools) = world.get_component_mut::<Tools>(agent) {
            tools.remove_owner(&task_id);
        }
        Ok(())
    })
    .await
}

impl jsonschema::Retrieve for NoExternalSchema {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema references are forbidden".into())
    }
}

async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, ToolError> {
    let mut bytes = Vec::new();
    let count = reader
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner pipe read failed"))?;
    if count == 0 {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(error(
            Kind::RunnerProtocolFailed,
            "runner frame lacks delimiter",
        ));
    }
    Ok(Some(bytes))
}

async fn read_json<T: DeserializeOwned, R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<T>, ToolError> {
    read_line(reader)
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
) -> Result<(), ToolError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| error(Kind::RunnerProtocolFailed, "protocol serialization failed"))?;
    bytes.push(b'\n');
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
                "MARGATROID_RUNNER_SCOPE",
                "MARGATROID_RUNNER_GLOBAL_CGROUP",
                "MARGATROID_RUNNER_GLOBAL_SLICE",
                "MARGATROID_RUNNER_TOTAL_MEMORY_BYTES",
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

async fn diagnostic_drain<R: AsyncRead + Unpin>(mut pipe: R) -> Result<(), ToolError> {
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe
            .read(&mut chunk)
            .await
            .map_err(|_| error(Kind::RunnerProtocolFailed, "runner diagnostics read failed"))?;
        if count == 0 {
            return Ok(());
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
        RunnerFrame::Log {
            request_id,
            mut record,
        } => {
            record.source = AgentLogSource::Tool;
            let result = persist_log(context, record).await.map(|()| Value::Null);
            (request_id, result)
        }
        RunnerFrame::LuaDiagnostic {
            request_id,
            mut record,
        } => {
            context.diagnostic_received = true;
            record.source = AgentLogSource::Tool;
            let original = record.raw_error.clone();
            if let Err(failure) = persist_log(context, record).await {
                eprintln!(
                    "tool automatic diagnostic persistence failed: {failure}; original: {}",
                    original.as_deref().unwrap_or("unavailable")
                );
            }
            (request_id, Ok(Value::Null))
        }
        RunnerFrame::Return { request_id, value } => {
            let result = parse_tool_result(&value).and_then(|response| {
                context.handle.return_result(Ok(response))?;
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
        RunnerFrame::Inject {
            request_id,
            definitions,
        } => {
            let result = inject_tools(context, definitions)
                .await
                .map(|()| Value::Null);
            (request_id, result)
        }
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
    let boundary = Some(
        MemoryBoundary::create(
            &context.memory_pool,
            request.limits.memory_bytes,
            request.limits.total_memory_bytes,
        )
        .await?,
    );
    boundary
        .as_ref()
        .expect("memory boundary required")
        .export(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            if let Some(boundary) = boundary {
                while boundary.cleanup().await.is_err() {
                    record_cleanup_failure(
                        context,
                        "failed runner start cleanup pending; supervision retained",
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
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
    let diagnostics = tokio::spawn(diagnostic_drain(stderr));
    let (host, mut outgoing) = mpsc::unbounded_channel::<HostFrame>();
    let writer = tokio::spawn(async move {
        write_json(&mut stdin, &request).await?;
        while let Some(frame) = outgoing.recv().await {
            write_json(
                &mut stdin,
                &HostWire {
                    protocol_version: 1,
                    frame,
                },
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
                    biased;
                    () = lifetime.lifetime_expired() => {
                        lifetime.cancel();
                        return Err(error(Kind::Timeout, "tool task lifetime exceeded"));
                    },
                    () = cancel.cancelled() => return Err(error(Kind::Cancelled, "tool task cancellation requested")),
                    frame = read_json::<RunnerWire, _>(&mut stdout) => {
                        let frame = frame?.ok_or_else(|| error(Kind::RunnerProtocolFailed, "runner exited without cleanup proof"))?;
                        if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "runner protocol version mismatch")); }
                        if let RunnerFrame::Configured { configuration } = frame.frame {
                            if context.configuration.is_some() { return Err(error(Kind::RunnerProtocolFailed, "duplicate tool configuration")); }
                            context.handle.configure_lifetime(configuration.timeout_ms)?;
                            context.configuration = Some(configuration);
                            host.send(HostFrame::Configured).map_err(|_| error(Kind::RunnerProtocolFailed, "configuration acknowledgement failed"))?;
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
                        if context.configuration.is_none() && !matches!(frame.frame, RunnerFrame::LuaDiagnostic { .. }) { return Err(error(Kind::RunnerProtocolFailed, "worker request before configuration")); }
                        let response = host_frame(context, frame.frame).await?;
                        host.send(response).map_err(|_| error(Kind::RunnerProtocolFailed, "runner input closed"))?;
                    }
                    value = context.input.recv(), if input_ready && context.configuration.is_some() => {
                        let Some(value) = value else { return Err(error(Kind::InputClosed, "task input channel closed")); };
                        host.send(HostFrame::Input { value }).map_err(|_| error(Kind::RunnerProtocolFailed, "runner input closed"))?;
                        input_ready = false;
                    }
                }
            }
        };
        exchange.await
    };
    if let Err(failure) = &result {
        report_host_failure(context, failure).await;
        complete_reply(context, Err(failure.clone()));
        context.handle.set_error(failure.clone());
    }
    if let Err(failure) = cleanup_owner(context).await {
        result = Err(failure);
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
                while let Some(frame) = read_json::<RunnerWire, _>(&mut stdout).await? {
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
    // Reap both pipe tasks and the boundary before propagating any pipe error.
    let writer_result = writer
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner input task failed"));
    let diagnostic_result = diagnostics
        .await
        .map_err(|_| error(Kind::RunnerProtocolFailed, "runner diagnostic task failed"));
    if let Some(boundary) = &boundary {
        while boundary.cleanup().await.is_err() {
            record_cleanup_failure(context, "cgroup cleanup incomplete; supervision retained");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    writer_result??;
    diagnostic_result??;
    if !proven {
        result = Err(error(Kind::CleanupFailed, "runner cleanup unproven"));
    }
    result
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
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut input = BufReader::new(tokio::io::stdin());
    let request = tokio::select! {
        request = read_json::<RunnerRequest, _>(&mut input) => request,
        _ = &mut shutdown => return 3,
    };
    let Ok(Some(request)) = request else {
        return 2;
    };
    if request.protocol_version != 1 || request.limits.validate().is_err() {
        return 2;
    }

    let Ok(executable) = std::env::current_exe() else {
        return 3;
    };
    if sandbox.is_some_and(|(backend, _)| !Path::new(backend).is_file()) {
        let frame = RunnerWire {
            protocol_version: 1,
            frame: RunnerFrame::Stopped {
                result: Err(error(
                    Kind::RunnerStartFailed,
                    "restricted worker could not start",
                )),
                cleanup_proven: true,
            },
        };
        let _ = write_json(&mut tokio::io::stdout(), &frame).await;
        return 0;
    }
    let boundary = match MemoryBoundary::inherited() {
        Ok(Some(boundary)) => boundary
            .validate(
                request.limits.memory_bytes,
                request.limits.total_memory_bytes,
            )
            .map(|()| boundary),
        Ok(None) => {
            MemoryBoundary::create(
                &Arc::default(),
                request.limits.memory_bytes,
                request.limits.total_memory_bytes,
            )
            .await
        }
        Err(failure) => Err(failure),
    };
    let boundary = match boundary {
        Ok(boundary) => boundary,
        Err(failure) => {
            let frame = RunnerWire {
                protocol_version: 1,
                frame: RunnerFrame::Stopped {
                    result: Err(failure),
                    cleanup_proven: true,
                },
            };
            let _ = write_json(&mut tokio::io::stdout(), &frame).await;
            return 3;
        }
    };
    let mut command = boundary.command(request.limits.memory_bytes, &executable, sandbox);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut output = tokio::io::stdout();
    let mut configuration: Option<ToolConfiguration> = None;
    let mut result = match command.spawn() {
        Err(_) => Err(error(
            Kind::RunnerStartFailed,
            "restricted worker could not start",
        )),
        Ok(mut worker) => {
            let mut worker_input = worker.stdin.take().expect("worker stdin piped");
            let mut worker_output =
                BufReader::new(worker.stdout.take().expect("worker stdout piped"));
            let exchange = async {
                write_json(&mut worker_input, &request).await?;
                let mut acknowledged = false;
                loop {
                    tokio::select! {
                        frame = read_json::<RunnerWire, _>(&mut worker_output) => {
                            let frame = frame?.ok_or_else(|| error(Kind::ExecutionFailed, "worker exited without terminal frame"))?;
                            if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "worker protocol version mismatch")); }
                            match frame.frame {
                                RunnerFrame::Configured { configuration: value } => {
                                    if configuration.is_some() { return Err(error(Kind::RunnerProtocolFailed, "duplicate worker configuration")); }
                                    value.validate()?;
                                    configuration = Some(value);
                                    write_json(&mut output, &frame).await?;
                                }
                                RunnerFrame::Done { result } => return result,
                                RunnerFrame::Stopped { .. } => return Err(error(Kind::RunnerProtocolFailed, "worker cannot claim descendant cleanup")),
                                RunnerFrame::LuaDiagnostic { .. } => write_json(&mut output, &frame).await?,
                                _ if acknowledged => write_json(&mut output, &frame).await?,
                                _ => return Err(error(Kind::RunnerProtocolFailed, "worker request before configuration acknowledgement")),
                            }
                        }
                        frame = read_json::<HostWire, _>(&mut input) => {
                            let frame = frame?.ok_or_else(|| error(Kind::Cancelled, "supervisor host input closed"))?;
                            if frame.protocol_version != 1 { return Err(error(Kind::RunnerProtocolFailed, "host protocol version mismatch")); }
                            if matches!(frame.frame, HostFrame::Configured) {
                                if acknowledged { return Err(error(Kind::RunnerProtocolFailed, "duplicate configuration acknowledgement")); }
                                configuration.ok_or_else(|| error(Kind::RunnerProtocolFailed, "configuration acknowledgement before configuration"))?;
                                acknowledged = true;
                            } else if !acknowledged && !matches!(frame.frame, HostFrame::Response { .. }) { return Err(error(Kind::RunnerProtocolFailed, "host frame before configuration acknowledgement")); }
                            write_json(&mut worker_input, &frame).await?;
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
            let oom = boundary.oom_killed().await;
            while boundary.kill().await.is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            while !cleanup_descendants().await {
                let frame = RunnerWire {
                    protocol_version: 1,
                    frame: RunnerFrame::Stopped {
                        result: Err(error(Kind::CleanupFailed, "supervisor cleanup pending")),
                        cleanup_proven: false,
                    },
                };
                let _ = write_json(&mut output, &frame).await;
            }
            let _ = worker.wait().await;
            let _ = tokio::io::copy(&mut worker_output, &mut tokio::io::sink()).await;
            if oom || boundary.oom_killed().await {
                Err(error(
                    Kind::ExecutionFailed,
                    "task aggregate memory limit exceeded",
                ))
            } else if configuration.is_none()
                && result
                    .as_ref()
                    .is_err_and(|failure| failure.kind == Kind::ExecutionFailed)
            {
                Err(error(Kind::SandboxUnavailable, "memory-enforced worker could not configure; ensure a delegated cgroup v2 memory controller or systemd user scope is available"))
            } else {
                result
            }
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
        let _ = write_json(&mut output, &frame).await;
    }
    while let Err(failure) = boundary.cleanup().await {
        result = Err(failure.clone());
        let frame = RunnerWire {
            protocol_version: 1,
            frame: RunnerFrame::Stopped {
                result: Err(failure),
                cleanup_proven: false,
            },
        };
        let _ = write_json(&mut output, &frame).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let frame = RunnerWire {
        protocol_version: 1,
        frame: RunnerFrame::Stopped {
            result,
            cleanup_proven: true,
        },
    };
    if write_json(&mut output, &frame).await.is_ok() {
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
    #[cfg(target_os = "linux")]
    if args
        .first()
        .is_some_and(|arg| arg == "--margatroid-tool-memory-entry")
    {
        return memory::enter_scope(&args[1..]);
    }
    if args != [std::ffi::OsString::from("--margatroid-tool-worker")] {
        return supervisor_main(args).await;
    }
    let mut input = BufReader::new(tokio::io::stdin());
    let Ok(Some(request)) = read_json::<RunnerRequest, _>(&mut input).await else {
        return 2;
    };
    if request.protocol_version != 1 || request.limits.validate().is_err() {
        return 2;
    }

    let result = run_tool_worker(request, input).await;
    let frame = RunnerWire {
        protocol_version: 1,
        frame: RunnerFrame::Done { result },
    };
    if write_json(&mut tokio::io::stdout(), &frame).await.is_ok() {
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
    input: mpsc::UnboundedSender<Value>,
) {
    let result = async {
        while let Some(frame) = read_json::<HostWire, _>(&mut reader).await? {
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
                    .send(value)
                    .map_err(|_| error(Kind::RunnerProtocolFailed, "Lua input closed"))?,
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

async fn run_tool_worker<R: AsyncBufRead + Unpin + Send + 'static>(
    request: RunnerRequest,
    input: R,
) -> Result<(), ToolError> {
    request.limits.validate()?;
    let mut input = input;
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(lua_failure)?;
    lua.set_memory_limit(request.limits.memory_bytes)
        .map_err(lua_failure)?;
    let deadline = Arc::new(Mutex::new(None::<Instant>));
    let hook_deadline = deadline.clone();
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
            Ok(VmState::Continue)
        },
    )
    .map_err(lua_failure)?;
    let globals = lua.globals();
    // Configuration runs with only the metadata API; execution capabilities are
    // installed after the host accepts the configuration.
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
    let arguments = data::to_lua(&lua, request.arguments.clone()).map_err(lua_failure)?;
    let context = data::to_lua(
        &lua,
        request
            .context
            .public_value(&request.arguments)
            .map_err(|failure| error(Kind::InvalidArguments, failure.to_string()))?,
    )
    .map_err(lua_failure)?;
    let host = host::Margatroid::install(
        &lua,
        host::HostDefinition {
            kind: host::HostKind::Tool,
            context: request
                .context
                .public_value(&request.arguments)
                .map_err(|_| error(Kind::InvalidRequest, "invalid tool context"))?,
            arguments: request.arguments.clone(),
        },
    )
    .map_err(lua_failure)?;
    // Registering async userdata methods may load the coroutine library in mlua.
    globals
        .set("coroutine", LuaValue::Nil)
        .map_err(lua_failure)?;
    let loaded = lua
        .load(&request.program)
        .set_name(format!("@{}", request.context.source_origin))
        .exec();
    if let Err(failure) = loaded {
        return Err(report_startup_failure(
            &request,
            &mut input,
            "load",
            failure,
            Kind::InvalidDefinition,
        )
        .await);
    }
    let configure = match globals.get::<mlua::Function>("configure") {
        Ok(configure) => configure,
        Err(failure) => {
            return Err(report_startup_failure(
                &request,
                &mut input,
                "configure",
                failure,
                Kind::InvalidDefinition,
            )
            .await)
        }
    };
    let configured: LuaValue = match configure.call((arguments.clone(), context.clone())) {
        Ok(value) => value,
        Err(failure) => {
            return Err(report_startup_failure(
                &request,
                &mut input,
                "configure",
                failure,
                Kind::InvalidDefinition,
            )
            .await)
        }
    };
    let configuration = match parse_configuration(&lua, configured) {
        Ok(value) => value,
        Err(failure) => {
            return Err(report_startup_failure(
                &request,
                &mut input,
                "configure",
                mlua::Error::external(failure),
                Kind::InvalidDefinition,
            )
            .await)
        }
    };
    write_json(
        &mut tokio::io::stdout(),
        &RunnerWire {
            protocol_version: 1,
            frame: RunnerFrame::Configured { configuration },
        },
    )
    .await?;
    let acknowledgement = read_json::<HostWire, _>(&mut input).await?;
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
    let package = lua.create_table().map_err(lua_failure)?;
    package
        .set(
            "config",
            format!("{}\n;\n?\n!\n-\n", std::path::MAIN_SEPARATOR),
        )
        .map_err(lua_failure)?;
    globals.set("package", package).map_err(lua_failure)?;
    let (sender, receiver) = mpsc::unbounded_channel();
    let finish = CancellationToken::new();
    let bridge = Arc::new(WorkerBridge {
        output: tokio::sync::Mutex::new(tokio::io::stdout()),
        pending: Mutex::new(BTreeMap::new()),
        input: tokio::sync::Mutex::new(receiver),
        sequence: AtomicU64::new(1),
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
                "use margatroid.process:start for subprocesses",
            )))
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    let os: mlua::Table = globals.get("os").map_err(lua_failure)?;
    os.set(
        "execute",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(error(
                Kind::InvalidRequest,
                "use margatroid.process:start for subprocesses",
            )))
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    os.set(
        "exit",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(error(
                Kind::InvalidRequest,
                "use margatroid.task:finish for task exit",
            )))
        })
        .map_err(lua_failure)?,
    )
    .map_err(lua_failure)?;
    let callbacks = lua.create_table().map_err(lua_failure)?;
    install_log_callback(&lua, &callbacks, bridge.clone(), request.context.clone())
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    callbacks
        .set(
            "task_append_output",
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
    callbacks
        .set(
            "task_replace_output",
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
    let inject_bridge = bridge.clone();
    callbacks
        .set(
            "tools_register",
            lua.create_async_function(move |lua, value: LuaValue| {
                let bridge = inject_bridge.clone();
                async move {
                    let definitions = lua.from_value::<Vec<InjectedToolDefinition>>(value)?;
                    bridge
                        .call(|request_id| RunnerFrame::Inject {
                            request_id,
                            definitions,
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
    callbacks
        .set(
            "task_return_result",
            lua.create_async_function(move |lua, value: LuaValue| {
                let bridge = task_bridge.clone();
                async move {
                    let value = lua.from_value::<Value>(value)?;
                    parse_tool_result(&value).map_err(mlua::Error::external)?;
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
    callbacks
        .set(
            "task_read_output",
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
                    data::to_lua(&lua, value)
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let task_bridge = bridge.clone();
    callbacks
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
    callbacks
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
    callbacks
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
                        )
                        .await;
                    });
                    data::to_lua(&lua, value)
                }
            })
            .map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    callbacks
        .set(
            "wait_any",
            host::install_wait_any(&lua).map_err(lua_failure)?,
        )
        .map_err(lua_failure)?;
    let (processes, process_start) =
        process::install_process(&lua, request.context.project_root.clone())
            .map_err(lua_failure)?;
    callbacks
        .set("process_start", process_start)
        .map_err(lua_failure)?;
    data::install(&lua, &callbacks, request.context.project_root.clone()).map_err(lua_failure)?;
    host::Margatroid::enable(&host, callbacks).map_err(lua_failure)?;
    let arguments = data::to_lua(&lua, request.arguments.clone()).map_err(lua_failure)?;
    let context = data::to_lua(
        &lua,
        request
            .context
            .public_value(&request.arguments)
            .map_err(|failure| error(Kind::InvalidArguments, failure.to_string()))?,
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
        result = execution => match result {
            Ok(()) => Ok(()),
            Err(failure) => {
                let brief = lua_failure(failure.clone());
                if should_diagnose(&brief) {
                    let record = diagnostic_record(&request.context, "execute", &failure, &brief);
                    if let Err(log_failure) = bridge.call(|request_id| RunnerFrame::LuaDiagnostic { request_id, record }).await {
                        eprintln!("tool diagnostic transport failed: {log_failure}; original: {failure}");
                    }
                }
                Err(brief)
            }
        },
        _ = wait_deadline(execution_deadline.map(tokio::time::Instant::from_std)) => Err(error(Kind::Timeout, "Lua execution deadline exceeded")),
    };
    host_reader.abort();
    let _ = host_reader.await;
    processes
        .cleanup()
        .await
        .map_err(|failure| error(Kind::CleanupFailed, failure.to_string()))?;
    result
}

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

#[cfg(test)]
mod world_sender_tests {
    use super::*;
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};
    use core_plugin::App;
    use tool_core::ToolExecutionLimits;
    use workspace_core::WorkspaceState;

    struct TestStorageDirectory {
        _directory: tempfile::TempDir,
    }
    impl core_plugin::Resource for TestStorageDirectory {}

    fn injection_fixture() -> (App, Arc<WorldClosureSender>, ToolExecutionContext) {
        use closure_plugin::WorldClosureExt;

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE);
        let agent = app.world_mut().spawn();
        let tasks = ToolTasks::new();
        let (reply, _) = oneshot::channel();
        let (handle, input) = ToolTaskHandle::new(
            &ToolExecutionLimits::default(),
            "call".into(),
            "source".into(),
            reply,
        )
        .unwrap();
        tasks.register("caller".into(), handle.clone()).unwrap();
        app.world_mut().insert_component(agent, tasks.clone());
        app.world_mut().insert_component(agent, Tools::new());
        let sender = Arc::new(app.world().closure_sender(RuntimePlugin::UPDATE));
        let storage_root = tempfile::tempdir().unwrap();
        let storage = storage_core::open_storage(
            &storage_root.path().join("agent.sqlite"),
            &storage_root.path().join("agent.toml"),
            "",
        )
        .unwrap();
        app.world_mut().insert_resource(TestStorageDirectory {
            _directory: storage_root,
        });
        let context = ToolExecutionContext {
            storage,
            agent_name: "test".into(),
            diagnostic_received: false,
            request: ToolExecuteRequest {
                id: "caller".into(),
                agent,
                turn: "turn".into(),
                alias: "source".into(),
                call_id: "call".into(),
                arguments: serde_json::json!({}),
                reply: Arc::new(Mutex::new(None)),
            },
            tool_entry: ToolEntry {
                path: PathBuf::from("/resource/tool.toml"),
                description: "source".into(),
                input_schema: serde_json::json!({}),
                owner_task_id: None,
                binding: None,
            },
            world: Arc::downgrade(&sender),
            sandbox_path: None,
            project_root: PathBuf::from("/project"),
            runner_path: PathBuf::from("/runner"),
            limits: ToolExecutionLimits::default(),
            memory_pool: Arc::default(),
            configuration: Some(ToolConfiguration { timeout_ms: 0 }),
            handle,
            tasks,
            input,
        };
        (app, sender, context)
    }

    async fn world_action<T>(app: &mut App, action: impl std::future::Future<Output = T>) -> T {
        tokio::pin!(action);
        if let Some(result) = action.as_mut().now_or_never() {
            return result;
        }
        app.tick();
        action.await
    }

    fn definition(alias: &str, owner: Option<&str>) -> InjectedToolDefinition {
        InjectedToolDefinition {
            alias: alias.into(),
            description: "injected".into(),
            input_schema: serde_json::json!({}),
            owner_task_id: owner.map(str::to_owned),
            binding: Some(serde_json::json!({"method": "echo"})),
        }
    }

    #[tokio::test]
    async fn static_injection_survives_caller_cleanup() {
        let (mut app, _sender, context) = injection_fixture();
        let omitted: InjectedToolDefinition = serde_json::from_value(serde_json::json!({
            "alias": "static", "description": "static", "input_schema": {},
            "binding": {"method": "echo"}
        }))
        .unwrap();
        assert_eq!(omitted.owner_task_id, None);
        world_action(&mut app, inject_tools(&context, vec![omitted]))
            .await
            .unwrap();
        world_action(&mut app, cleanup_owner(&context))
            .await
            .unwrap();
        let entry = app
            .world()
            .get_component::<Tools>(context.request.agent)
            .unwrap()
            .get("static")
            .unwrap();
        assert_eq!(entry.owner_task_id, None);
        assert_eq!(entry.path, context.tool_entry.path);
        assert_eq!(entry.binding, Some(serde_json::json!({"method": "echo"})));
        assert!(context.handle.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn explicit_other_owner_controls_record_cleanup_without_cancelling_calls() {
        let (mut app, _sender, mut context) = injection_fixture();
        let caller = context.handle.clone();
        let (reply, _) = oneshot::channel();
        let (owner, _) = ToolTaskHandle::new(
            &context.limits,
            "owner-call".into(),
            "owner-alias".into(),
            reply,
        )
        .unwrap();
        context
            .tasks
            .register("other".into(), owner.clone())
            .unwrap();
        world_action(
            &mut app,
            inject_tools(&context, vec![definition("owned", Some("other"))]),
        )
        .await
        .unwrap();
        world_action(&mut app, cleanup_owner(&context))
            .await
            .unwrap();
        assert_eq!(
            app.world()
                .get_component::<Tools>(context.request.agent)
                .unwrap()
                .get("owned")
                .unwrap()
                .owner_task_id
                .as_deref(),
            Some("other")
        );
        // The accepted caller is an independent task even if its tool record is owned.
        let (reply, _) = oneshot::channel();
        let (accepted, _) = ToolTaskHandle::new(
            &context.limits,
            "accepted-call".into(),
            "owned".into(),
            reply,
        )
        .unwrap();
        context
            .tasks
            .register("accepted".into(), accepted.clone())
            .unwrap();
        context.request.id = "other".into();
        context.handle = owner.clone();
        world_action(&mut app, cleanup_owner(&context))
            .await
            .unwrap();
        assert!(app
            .world()
            .get_component::<Tools>(context.request.agent)
            .unwrap()
            .get("owned")
            .is_none());
        assert!(owner.cancel_token().is_cancelled());
        assert!(caller.cancel_token().is_cancelled());
        assert!(!accepted.cancel_token().is_cancelled());
        assert!(!accepted.has_returned());
    }

    #[tokio::test]
    async fn invalid_owner_or_alias_collision_rejects_the_whole_batch() {
        let (mut app, _sender, context) = injection_fixture();
        world_action(
            &mut app,
            inject_tools(&context, vec![definition("existing", None)]),
        )
        .await
        .unwrap();
        let original = app
            .world()
            .get_component::<Tools>(context.request.agent)
            .unwrap()
            .clone();
        for (bad, kind) in [
            (
                definition("missing", Some("missing-owner")),
                Kind::TaskMissing,
            ),
            (
                definition("existing", Some("caller")),
                Kind::InvalidDefinition,
            ),
        ] {
            let failure = world_action(
                &mut app,
                inject_tools(&context, vec![definition("new", None), bad]),
            )
            .await
            .unwrap_err();
            assert_eq!(failure.kind, kind);
            assert_eq!(
                app.world()
                    .get_component::<Tools>(context.request.agent)
                    .unwrap(),
                &original
            );
        }
        let (reply, _) = oneshot::channel();
        let (cancelled_owner, _) =
            ToolTaskHandle::new(&context.limits, "cancelled".into(), "owner".into(), reply)
                .unwrap();
        context
            .tasks
            .register("cancelled".into(), cancelled_owner.clone())
            .unwrap();
        cancelled_owner.cancel();
        let failure = world_action(
            &mut app,
            inject_tools(
                &context,
                vec![
                    definition("new", None),
                    definition("cancelled-owned", Some("cancelled")),
                ],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.kind, Kind::InvalidRequest);
        assert_eq!(
            app.world()
                .get_component::<Tools>(context.request.agent)
                .unwrap(),
            &original
        );
        context.handle.cancel();
        let failure = world_action(
            &mut app,
            inject_tools(&context, vec![definition("new", None)]),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.kind, Kind::Cancelled);
        assert_eq!(
            app.world()
                .get_component::<Tools>(context.request.agent)
                .unwrap(),
            &original
        );
    }

    #[test]
    fn app_drop_releases_queued_tool_launch_and_its_reply() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(ToolPlugin::new());
        let root = tempfile::tempdir().unwrap();
        let agent = app.world_mut().spawn();
        let storage = storage_core::open_storage(
            &root.path().join("agent.sqlite"),
            &root.path().join("agent.toml"),
            "",
        )
        .unwrap();
        app.world_mut().insert_component(agent, storage);
        app.world_mut().insert_component(
            agent,
            Agent {
                name: "test".into(),
                workspace: agent,
                state: AgentState::Working,
                turn: Some("turn".into()),
            },
        );
        app.world_mut().insert_component(
            agent,
            Workspace {
                definition: None,
                name: "test".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().to_owned(),
                state: WorkspaceState::Running,
                agents: Default::default(),
            },
        );
        let mut tools = Tools::new();
        tools
            .insert(
                "test".into(),
                ToolEntry {
                    path: root.path().join("tool.toml"),
                    description: "test".into(),
                    input_schema: serde_json::json!({}),
                    owner_task_id: None,
                    binding: None,
                },
            )
            .unwrap();
        app.world_mut().insert_component(agent, tools);
        let tasks = ToolTasks::new();
        app.world_mut().insert_component(agent, tasks.clone());
        let sender = Arc::downgrade(&app.world().get_resource::<crate::ToolRuntime>().unwrap().0);
        let (reply, mut receiver) = oneshot::channel();
        handle_tool_execute(
            app.world_mut(),
            ToolExecuteRequest {
                id: "queued".into(),
                agent,
                turn: "turn".into(),
                alias: "test".into(),
                call_id: "call".into(),
                arguments: serde_json::json!({}),
                reply: Arc::new(Mutex::new(Some(reply))),
            },
        );
        assert_eq!(tasks.list().len(), 1);
        drop(app);
        assert!(sender.upgrade().is_none());
        assert_eq!(
            receiver.try_recv().unwrap().unwrap_err().kind,
            Kind::SubmitFailed
        );
        assert!(tasks.list().is_empty());
    }

    #[test]
    fn optional_context_markers_are_lua_nil_and_present_markers_survive_the_wire() {
        let lua = Lua::new();
        let mut context = ToolContext {
            agent_name: "test".into(),
            turn: "turn".into(),
            source_origin: "/resource/main.lua".into(),
            task_id: "task".into(),
            call_id: "call".into(),
            alias: "alias".into(),
            owner_task_id: None,
            binding: None,
            resource_dir: PathBuf::from("/resource"),
            project_root: PathBuf::from("/project"),
            temp_dir: PathBuf::from("/temp"),
        };
        let wire = serde_json::to_vec(&context).unwrap();
        let received: ToolContext = serde_json::from_slice(&wire).unwrap();
        let arguments = serde_json::json!({"message": "hello"});
        let public_context = received.public_value(&arguments).unwrap();
        let LuaValue::Table(table) = data::to_lua(&lua, public_context.clone()).unwrap() else {
            panic!("context table");
        };
        assert!(matches!(
            table.get::<LuaValue>("owner_task_id").unwrap(),
            LuaValue::Nil
        ));
        assert!(matches!(
            table.get::<LuaValue>("binding").unwrap(),
            LuaValue::Nil
        ));
        context.owner_task_id = Some("owner".into());
        context.binding = Some(serde_json::json!({"method": "echo"}));
        let wire = serde_json::to_vec(&context).unwrap();
        let received: ToolContext = serde_json::from_slice(&wire).unwrap();
        let arguments = serde_json::json!({"message": "hello"});
        let public_context = received.public_value(&arguments).unwrap();
        let LuaValue::Table(table) = data::to_lua(&lua, public_context.clone()).unwrap() else {
            panic!("context table");
        };
        assert_eq!(
            table
                .get::<mlua::Table>("arguments")
                .unwrap()
                .get::<String>("message")
                .unwrap(),
            "hello"
        );
        assert_eq!(table.get::<String>("project_root").unwrap(), "/project");
        host::Margatroid::install(
            &lua,
            host::HostDefinition {
                kind: host::HostKind::Tool,
                context: public_context,
                arguments: serde_json::json!({"message": "host arguments"}),
            },
        )
        .unwrap();
        lua.load(
            r#"
            assert(margatroid.context.kind == "tool")
            assert(margatroid.context.agent_info.name == "test")
            assert(margatroid.context.project_root == "/project")
            assert(margatroid.context.arguments.message == "host arguments")
            assert(not pcall(function() margatroid.context = {} end))
            local context = margatroid.context
            context.arguments.message = "changed"
            assert(margatroid.context.arguments.message == "host arguments")
            assert(margatroid.arguments == nil)
            assert(margatroid.version == nil)
            assert(margatroid.null == nil)
            "#,
        )
        .exec()
        .unwrap();
        assert_eq!(table.get::<String>("owner_task_id").unwrap(), "owner");
        assert_eq!(
            table
                .get::<mlua::Table>("binding")
                .unwrap()
                .get::<String>("method")
                .unwrap(),
            "echo"
        );
    }

    #[test]
    fn program_reads_main_lua_beside_the_manifest_without_a_byte_limit() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("tool.toml");
        // Neither a manifest nor a registration schema is needed at execution time.
        let program = format!("-- {}\nreturn 1", "x".repeat(1024 * 1024));
        std::fs::write(root.path().join("main.lua"), &program).unwrap();
        assert_eq!(resource_dir(&manifest).unwrap(), root.path());
        assert_eq!(read_program(&manifest).unwrap(), program);
        std::fs::write(root.path().join("main.lua"), [0xff]).unwrap();
        assert_eq!(read_program(&manifest).unwrap_err().kind, Kind::ReadFailed);
        assert_eq!(
            read_program(&root.path().join("missing/tool.toml"))
                .unwrap_err()
                .kind,
            Kind::ReadFailed
        );
    }
}
