//! Raw subprocess pipes owned by one sandboxed Lua worker.
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{Lua, UserData, UserDataMethods};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio_util::sync::CancellationToken;
use tool_core::{ToolError, ToolErrorKind as Kind, ToolExecutionLimits};

const READ_BYTES: usize = 8192;

fn failure(kind: Kind, message: impl Into<String>) -> ToolError {
    ToolError::new(kind, message)
}
fn io_failure(operation: &str, error: std::io::Error) -> ToolError {
    failure(
        Kind::ExecutionFailed,
        format!("process {operation}: {error}"),
    )
}
fn closed() -> ToolError {
    failure(Kind::Closed, "process session closed")
}

#[derive(Default)]
struct Sessions {
    closed: bool,
    entries: Vec<ProcessSession>,
}

/// Only this worker's sessions; no process data crosses the runner protocol.
#[derive(Clone, Default)]
pub(crate) struct ProcessRegistry(Arc<Mutex<Sessions>>);

impl ProcessRegistry {
    /// Call on every worker exit, after dropping the active Lua execution future.
    pub(crate) async fn cleanup(&self) -> Result<(), ToolError> {
        use futures_util::{stream::FuturesUnordered, StreamExt};
        let sessions = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            state.entries.clone()
        };
        for session in &sessions {
            session.inner.close.cancel();
        }
        let mut pending: FuturesUnordered<_> = sessions.iter().map(ProcessSession::close).collect();
        let mut failed = None;
        while let Some(result) = pending.next().await {
            if let Err(error) = result {
                failed.get_or_insert(error);
            }
        }
        if let Some(error) = failed {
            return Err(error);
        }
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .clear();
        Ok(())
    }
}

#[derive(Clone)]
struct ProcessSession {
    inner: Arc<ProcessInner>,
}
struct ProcessInner {
    stdin: AsyncMutex<WritePipe>,
    stdout: AsyncMutex<Option<ChildStdout>>,
    stderr: AsyncMutex<Option<ChildStderr>>,
    close: CancellationToken,
    status: watch::Receiver<Option<Result<ExitStatus, ToolError>>>,
    stdout_budget: Arc<Budget>,
    stderr_budget: Arc<Budget>,
    write_limit: usize,
}
struct WritePipe {
    stdin: Option<ChildStdin>,
    pending: Option<PendingWrite>,
}
struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
}
struct Budget {
    used: AtomicUsize,
    limit: usize,
}
impl Budget {
    fn new(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit,
        }
    }
    fn consume(&self, count: usize) -> Result<(), ToolError> {
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(count).filter(|total| *total <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| failure(Kind::OutputTooLarge, "process pipe exceeds byte budget"))
    }
}

impl ProcessSession {
    async fn wait(&self) -> Result<ExitStatus, ToolError> {
        let mut status = self.inner.status.clone();
        loop {
            if let Some(result) = status.borrow_and_update().clone() {
                return result;
            }
            status.changed().await.map_err(|_| {
                failure(
                    Kind::CleanupFailed,
                    "process waiter stopped without exit status",
                )
            })?;
        }
    }

    async fn close(&self) -> Result<(), ToolError> {
        self.inner.close.cancel();
        let result = self.wait().await;
        {
            let mut pipe = self.inner.stdin.lock().await;
            pipe.stdin.take();
            pipe.pending.take();
        }
        self.inner.stdout.lock().await.take();
        self.inner.stderr.lock().await.take();
        result.map(|_| ())
    }

    async fn write(&self, bytes: &[u8]) -> Result<(), ToolError> {
        if bytes.len() > self.inner.write_limit {
            return Err(failure(
                Kind::OutputTooLarge,
                "process write exceeds byte budget",
            ));
        }
        tokio::select! {
            biased;
            _ = self.inner.close.cancelled() => Err(closed()),
            result = async {
                let mut pipe = self.inner.stdin.lock().await;
                if pipe.stdin.is_none() {
                    return Err(closed());
                }
                if let Some(pending) = &pipe.pending {
                    if pending.bytes != bytes {
                        return Err(failure(
                            Kind::InvalidRequest,
                            "process has an unfinished write; retry the same complete bytes",
                        ));
                    }
                } else {
                    pipe.pending = Some(PendingWrite { bytes: bytes.to_vec(), offset: 0 });
                }
                let WritePipe { stdin, pending } = &mut *pipe;
                let stdin = stdin.as_mut().ok_or_else(closed)?;
                let frame = pending.as_mut().expect("pending frame initialized");
                while frame.offset < frame.bytes.len() {
                    // write is cancellation-safe. Once it returns, advance the
                    // durable offset before any further await can yield.
                    let count = stdin.write(&frame.bytes[frame.offset..]).await
                        .map_err(|error| io_failure("write", error))?;
                    if count == 0 {
                        return Err(io_failure("write", std::io::Error::new(
                            std::io::ErrorKind::WriteZero, "process stdin wrote zero bytes",
                        )));
                    }
                    frame.offset += count;
                }
                *pending = None;
                Ok(())
            } => result,
        }
    }
}

async fn read_pipe<R: AsyncRead + Unpin>(
    pipe: &AsyncMutex<Option<R>>,
    budget: &Budget,
    close: &CancellationToken,
) -> Result<(Vec<u8>, bool), ToolError> {
    tokio::select! {
        biased;
        _ = close.cancelled() => Err(closed()),
        result = async {
            let mut pipe = pipe.lock().await;
            let pipe = pipe.as_mut().ok_or_else(closed)?;
            let mut data = vec![0; READ_BYTES];
            let count = pipe.read(&mut data).await.map_err(|error| io_failure("read", error))?;
            budget.consume(count)?;
            data.truncate(count);
            Ok((data, count == 0))
        } => result,
    }
}

impl UserData for ProcessSession {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function(
            "write",
            |_, (userdata, bytes): (mlua::AnyUserData, mlua::String)| {
                let session = userdata
                    .borrow::<ProcessSession>()
                    .map(|session| session.clone());
                async move {
                    session?
                        .write(&bytes.as_bytes())
                        .await
                        .map_err(mlua::Error::external)
                }
            },
        );
        methods.add_async_function("read", |lua, userdata: mlua::AnyUserData| {
            let session = userdata
                .borrow::<ProcessSession>()
                .map(|session| session.clone());
            async move {
                let session = session?;
                let (data, eof) = read_pipe(
                    &session.inner.stdout,
                    &session.inner.stdout_budget,
                    &session.inner.close,
                )
                .await
                .map_err(mlua::Error::external)?;
                let result = lua.create_table()?;
                result.set("data", lua.create_string(data)?)?;
                result.set("eof", eof)?;
                Ok(result)
            }
        });
        methods.add_async_function("read_stderr", |lua, userdata: mlua::AnyUserData| {
            let session = userdata
                .borrow::<ProcessSession>()
                .map(|session| session.clone());
            async move {
                let session = session?;
                let (data, eof) = read_pipe(
                    &session.inner.stderr,
                    &session.inner.stderr_budget,
                    &session.inner.close,
                )
                .await
                .map_err(mlua::Error::external)?;
                let result = lua.create_table()?;
                result.set("data", lua.create_string(data)?)?;
                result.set("eof", eof)?;
                Ok(result)
            }
        });
        methods.add_async_function("wait", |lua, userdata: mlua::AnyUserData| {
            let session = userdata
                .borrow::<ProcessSession>()
                .map(|session| session.clone());
            async move {
                let status = session?.wait().await.map_err(mlua::Error::external)?;
                let result = lua.create_table()?;
                result.set("exit_code", status.code())?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    result.set("signal", status.signal())?;
                }
                Ok(result)
            }
        });
        methods.add_async_function("close", |_, userdata: mlua::AnyUserData| {
            let session = userdata
                .borrow::<ProcessSession>()
                .map(|session| session.clone());
            async move { session?.close().await.map_err(mlua::Error::external) }
        });
    }
}

/// Install only after configuration acknowledgement, inside the sandbox worker.
/// Relative executable paths resolve against project_root; PATH lookup is forbidden.
pub(crate) fn install_process(
    lua: &Lua,
    project_root: PathBuf,
    limits: ToolExecutionLimits,
) -> Result<ProcessRegistry, ToolError> {
    limits.validate()?;
    let registry = ProcessRegistry::default();
    let sessions = registry.clone();
    let stdout_budget = Arc::new(Budget::new(limits.output_bytes));
    let stderr_budget = Arc::new(Budget::new(limits.diagnostic_bytes));
    let constructor = lua.create_async_function(move |lua, options: mlua::Table| {
        let sessions = sessions.clone();
        let root = project_root.clone();
        let stdout_budget = stdout_budget.clone();
        let stderr_budget = stderr_budget.clone();
        async move {
            let program: String = options.get("program")?;
            let arguments: Option<Vec<String>> = options.get("arguments")?;
            let arguments = arguments.unwrap_or_default();
            if program.is_empty() || program.contains('\0') || arguments.iter().any(|arg| arg.contains('\0')) {
                return Err(mlua::Error::external(failure(Kind::InvalidArguments, "invalid process program or arguments")));
            }
            if program.len().saturating_add(arguments.iter().map(String::len).sum::<usize>()) > limits.argument_bytes {
                return Err(mlua::Error::external(failure(Kind::OutputTooLarge, "process arguments exceed byte budget")));
            }
            let program = Path::new(&program);
            let executable = if program.is_absolute() { program.to_owned() } else { root.join(program) };
            let mut state = sessions.0.lock().unwrap_or_else(|error| error.into_inner());
            if state.closed {
                return Err(mlua::Error::external(closed()));
            }
            let mut child = Command::new(executable)
                .args(arguments).current_dir(root)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
                .kill_on_drop(true).spawn()
                .map_err(|error| mlua::Error::external(failure(Kind::RunnerStartFailed, format!("process spawn: {error}"))))?;
            let (status_tx, status) = watch::channel(None);
            let close = CancellationToken::new();
            let session = ProcessSession { inner: Arc::new(ProcessInner {
                stdin: AsyncMutex::new(WritePipe { stdin: child.stdin.take(), pending: None }),
                stdout: AsyncMutex::new(child.stdout.take()),
                stderr: AsyncMutex::new(child.stderr.take()),
                close: close.clone(), status,
                stdout_budget, stderr_budget,
                write_limit: limits.argument_bytes,
            }) };
            tokio::spawn(async move {
                let result = tokio::select! {
                    biased;
                    result = child.wait() => result.map_err(|error| io_failure("wait", error)),
                    _ = close.cancelled() => {
                        match child.start_kill() {
                            Ok(()) => child.wait().await.map_err(|error| failure(Kind::CleanupFailed, format!("process reap: {error}"))),
                            Err(error) => match child.wait().await {
                                Ok(status) => Ok(status),
                                Err(_) => Err(failure(Kind::CleanupFailed, format!("process kill: {error}"))),
                            }
                        }
                    }
                };
                status_tx.send_replace(Some(result));
            });
            state.entries.push(session.clone());
            drop(state);
            lua.create_userdata(session)
        }
    }).map_err(|error| failure(Kind::ExecutionFailed, error.to_string()))?;
    lua.globals()
        .set("process_open", constructor)
        .map_err(|error| failure(Kind::ExecutionFailed, error.to_string()))?;
    Ok(registry)
}

#[cfg(test)]
#[path = "../../tests/internal/process.rs"]
mod tests;
