use crate::{RunnerRequest, RunnerResponse, ToolError, ToolErrorKind};
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn error(kind: ToolErrorKind, message: impl Into<String>) -> ToolError {
    ToolError::new(kind, message)
}
async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: R, limit: usize) -> mlua::Result<String> {
    let mut bytes = Vec::new();
    pipe.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(mlua::Error::external)?;
    if bytes.len() > limit {
        return Err(mlua::Error::external("shell output exceeds limit"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
async fn lua_execute(
    request: &RunnerRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<String, ToolError> {
    request.limits.validate()?;
    if request.protocol_version != 1 || request.script.len() > request.limits.script_bytes {
        return Err(error(
            ToolErrorKind::InvalidRequest,
            "invalid runner request",
        ));
    }
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    )
    .map_err(|_| error(ToolErrorKind::ExecutionFailed, "Lua initialization failed"))?;
    lua.set_memory_limit(request.limits.memory_bytes)
        .map_err(|_| error(ToolErrorKind::ExecutionFailed, "Lua memory limit failed"))?;
    let deadline = Instant::now() + request.limits.execution_timeout;
    let budget = request.limits.instruction_budget;
    let count = std::sync::atomic::AtomicU64::new(0);
    let hook_cancel = cancelled.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            if hook_cancel.load(Ordering::Acquire) {
                return Err(mlua::Error::external("cancelled"));
            }
            if Instant::now() >= deadline {
                return Err(mlua::Error::external("deadline exceeded"));
            }
            if count
                .fetch_add(1000, Ordering::Relaxed)
                .saturating_add(1000)
                > budget
            {
                return Err(mlua::Error::external("instruction budget exceeded"));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(|_| {
        error(
            ToolErrorKind::ExecutionFailed,
            "Lua hook installation failed",
        )
    })?;
    let globals = lua.globals();
    globals
        .set(
            "print",
            lua.create_function(|_, args: mlua::MultiValue| {
                let message = args
                    .into_iter()
                    .map(|value| match value {
                        Value::String(s) => s.to_string_lossy(),
                        v => format!("{v:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join("\t");
                eprintln!("{message}");
                Ok(())
            })
            .map_err(|e| error(ToolErrorKind::ExecutionFailed, e.to_string()))?,
        )
        .map_err(|e| error(ToolErrorKind::ExecutionFailed, e.to_string()))?;
    let json = lua
        .create_table()
        .map_err(|e| error(ToolErrorKind::ExecutionFailed, e.to_string()))?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            let value: serde_json::Value = lua.from_value(value)?;
            serde_json::to_string(&value).map_err(mlua::Error::external)
        })
        .unwrap(),
    )
    .unwrap();
    json.set(
        "decode",
        lua.create_function(|lua, value: String| {
            let value: serde_json::Value =
                serde_json::from_str(&value).map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })
        .unwrap(),
    )
    .unwrap();
    globals.set("json", json).unwrap();
    let output_limit = request.limits.output_bytes;
    let diagnostic_limit = request.limits.diagnostic_bytes;
    let host_timeout = request.limits.host_call_timeout;
    let project_root = request.context.project_root.clone();
    let shell_cancel = cancelled.clone();
    globals.set("shell", lua.create_async_function(move |lua, (script, options): (String, Option<mlua::Table>)| {
        let project_root = project_root.clone();
        let cancelled = shell_cancel.clone();
        async move {
            let use_pty = options.as_ref().is_some_and(|t| t.get::<bool>("pty").unwrap_or(false));
            #[cfg(not(target_os = "linux"))]
            if use_pty { return Err(mlua::Error::external("PTY requires Linux")); }
            let mut command = tokio::process::Command::new("/bin/sh");
            command.arg("-c").arg(script).current_dir(project_root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            crate::process::filtered_environment(&mut command);
            #[cfg(target_os = "linux")]
            let terminal = if use_pty { Some(crate::pty::configure(&mut command).map_err(mlua::Error::external)?) } else { None };
            let mut child = command.spawn().map_err(mlua::Error::external)?;
            drop(command);
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let remaining = deadline.saturating_duration_since(Instant::now()).min(host_timeout);
            let io = async {
                let output = async {
                    #[cfg(target_os = "linux")]
                    if let Some(terminal) = terminal { return Ok::<_, mlua::Error>((crate::pty::drain(terminal, output_limit).await?, String::new())); }
                    let (stdout, stderr) = tokio::try_join!(read_pipe(stdout.unwrap(), output_limit), read_pipe(stderr.unwrap(), diagnostic_limit))?;
                    Ok((stdout, stderr))
                };
                let (status, (stdout, stderr)) = tokio::try_join!(async { child.wait().await.map_err(mlua::Error::external) }, output)?;
                let table = lua.create_table()?;
                table.set("stdout", stdout)?; table.set("stderr", stderr)?; table.set("code", status.code().unwrap_or(-1))?;
                Ok::<_, mlua::Error>(table)
            };
            let cancellation = async { loop { if cancelled.load(Ordering::Acquire) { return; } tokio::time::sleep(Duration::from_millis(10)).await; } };
            let result = tokio::select! {
                result = tokio::time::timeout(remaining, io) => result.map_err(|_| mlua::Error::external("shell deadline exceeded"))?,
                _ = cancellation => Err(mlua::Error::external("cancelled")),
            };
            if result.is_err() { let _ = child.start_kill(); let _ = child.wait().await; }
            result
        }
    }).map_err(|e| error(ToolErrorKind::ExecutionFailed, e.to_string()))?).unwrap();
    globals
        .set(
            "handoff",
            lua.create_function(|_, _: mlua::MultiValue| {
                Err::<(), _>(mlua::Error::external("UnsupportedHandoff"))
            })
            .unwrap(),
        )
        .unwrap();
    let arguments = lua
        .to_value(&request.arguments)
        .map_err(|e| error(ToolErrorKind::InvalidArguments, e.to_string()))?;
    let context = lua
        .to_value(&request.context)
        .map_err(|e| error(ToolErrorKind::InvalidRequest, e.to_string()))?;
    globals.set("args", arguments.clone()).unwrap();
    globals.set("context", context.clone()).unwrap();
    let execution = async {
        lua.load(&request.script)
            .set_name("tool")
            .exec_async()
            .await?;
        let execute: mlua::Function = globals.get("execute")?;
        let value: Value = execute.call_async((arguments, context)).await?;
        match value {
            Value::String(s) => s.to_str().map(|s| s.to_string()),
            _ => Err(mlua::Error::external("execute must return a String")),
        }
    };
    let result = execution.await.map_err(|e| {
        let message = e.to_string();
        let kind = if cancelled.load(Ordering::Acquire) {
            ToolErrorKind::Cancelled
        } else if message.contains("UnsupportedHandoff") {
            ToolErrorKind::UnsupportedHandoff
        } else if message.contains("deadline exceeded") {
            ToolErrorKind::Timeout
        } else if message.contains("output exceeds limit") {
            ToolErrorKind::OutputTooLarge
        } else {
            ToolErrorKind::ExecutionFailed
        };
        error(kind, message)
    })?;
    if result.len() > output_limit {
        return Err(error(
            ToolErrorKind::OutputTooLarge,
            "tool output exceeds limit",
        ));
    }
    Ok(result)
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
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, suffix)) = stat.rsplit_once(") ") else {
            continue;
        };
        if let Some(ppid) = suffix
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<i32>().ok())
        {
            parents.insert(pid, ppid);
        }
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
async fn cleanup(timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let Ok(children) = descendants() else {
            return false;
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
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[doc(hidden)]
pub async fn runner_main() -> i32 {
    #[cfg(target_os = "linux")]
    if nix::sys::prctl::set_child_subreaper(true).is_err() {
        return 3;
    }
    #[cfg(not(target_os = "linux"))]
    return 3;
    if let Some(path) = std::env::var_os("MARGATROID_RUNNER_CGROUP") {
        if tokio::fs::write(
            std::path::PathBuf::from(path).join("cgroup.procs"),
            std::process::id().to_string(),
        )
        .await
        .is_err()
        {
            return 3;
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal_cancel = cancelled.clone();
    let signal = tokio::spawn(async move {
        #[cfg(unix)]
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
            signal_cancel.store(true, Ordering::Release);
        }
    });
    let mut raw = Vec::new();
    let input = tokio::io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut raw)
        .await;
    let request = if input.is_ok() && raw.len() <= 4 * 1024 * 1024 {
        serde_json::from_slice::<RunnerRequest>(&raw).ok()
    } else {
        None
    };
    let Some(request) = request else {
        signal.abort();
        return 2;
    };
    let result = lua_execute(&request, cancelled).await;
    let cleanup_proven = cleanup(request.limits.cleanup_timeout).await;
    let result = if cleanup_proven {
        result
    } else {
        Err(error(
            ToolErrorKind::CleanupFailed,
            "runner descendant cleanup failed",
        ))
    };
    let response = RunnerResponse {
        protocol_version: 1,
        result,
        cleanup_proven,
    };
    let bytes = serde_json::to_vec(&response).unwrap();
    let result = tokio::io::stdout().write_all(&bytes).await;
    signal.abort();
    if result.is_ok() {
        0
    } else {
        3
    }
}
