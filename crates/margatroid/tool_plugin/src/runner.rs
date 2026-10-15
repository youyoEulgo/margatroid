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
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::COROUTINE
            | StdLib::IO
            | StdLib::OS,
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
    globals.set("json", json.clone()).unwrap();
    let package = lua.create_table().unwrap();
    package
        .set(
            "config",
            format!("{}\n;\n?\n!\n-\n", std::path::MAIN_SEPARATOR),
        )
        .unwrap();
    globals.set("package", package).unwrap();
    let io: mlua::Table = globals.get("io").unwrap();
    let diagnostic_stream: Value = io.get("stderr").unwrap();
    io.get::<mlua::Function>("output")
        .unwrap()
        .call::<()>(diagnostic_stream.clone())
        .unwrap();
    io.set("stdout", diagnostic_stream).unwrap();
    io.set(
        "popen",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external(
                "use margatroid().shell for subprocesses",
            ))
        })
        .unwrap(),
    )
    .unwrap();
    let os: mlua::Table = globals.get("os").unwrap();
    os.set(
        "exit",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::external("os.exit is unavailable in tools"))
        })
        .unwrap(),
    )
    .unwrap();
    let output_limit = request.limits.output_bytes;
    let diagnostic_limit = request.limits.diagnostic_bytes;
    let host_timeout = request.limits.host_call_timeout;
    let project_root = request.context.project_root.clone();
    let shell_cancel = cancelled.clone();
    let shell = lua.create_async_function(move |lua, (script, options): (String, Option<mlua::Table>)| {
        let project_root = project_root.clone();
        let cancelled = shell_cancel.clone();
        async move {
            async fn capture<R: tokio::io::AsyncRead + Unpin>(mut pipe: R, limit: usize) -> mlua::Result<(String, bool)> {
                let mut bytes = Vec::new();
                let mut truncated = false;
                let mut chunk = [0u8; 8192];
                loop {
                    let count = pipe.read(&mut chunk).await.map_err(mlua::Error::external)?;
                    if count == 0 { break; }
                    let retained = limit.saturating_sub(bytes.len()).min(count);
                    bytes.extend_from_slice(&chunk[..retained]);
                    truncated |= retained < count;
                }
                Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
            }
            if script.trim().is_empty() { return Err(mlua::Error::external("shell command is empty")); }
            let requested_limit = options.as_ref().map(|t| t.get::<Option<usize>>("max_output_bytes")).transpose()?.flatten().unwrap_or(output_limit);
            let capture_limit = requested_limit.min(output_limit);
            let requested_timeout = options.as_ref().map(|t| t.get::<Option<u64>>("timeout_ms")).transpose()?.flatten().map(Duration::from_millis).unwrap_or(host_timeout);
            let use_pty = options.as_ref().map(|t| t.get::<Option<bool>>("pty")).transpose()?.flatten().unwrap_or(false);
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
            let remaining = deadline.saturating_duration_since(Instant::now()).min(host_timeout).min(requested_timeout);
            let io = async {
                let output = async {
                    #[cfg(target_os = "linux")]
                    if let Some(terminal) = terminal {
                        let (stdout, truncated) = crate::pty::drain(terminal, capture_limit).await?;
                        return Ok::<_, mlua::Error>((stdout, String::new(), truncated));
                    }
                    let ((stdout, stdout_truncated), (stderr, stderr_truncated)) = tokio::try_join!(capture(stdout.unwrap(), capture_limit), capture(stderr.unwrap(), diagnostic_limit.min(capture_limit)))?;
                    Ok((stdout, stderr, stdout_truncated || stderr_truncated))
                };
                let (status, (stdout, stderr, truncated)) = tokio::try_join!(async { child.wait().await.map_err(mlua::Error::external) }, output)?;
                let table = lua.create_table()?;
                let code = status.code().unwrap_or(-1);
                table.set("stdout", stdout)?;
                table.set("stderr", stderr)?;
                table.set("code", code)?;
                table.set("exit_code", code)?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    table.set("signal", status.signal())?;
                }
                table.set("truncated", truncated)?;
                table.set("terminal", use_pty)?;
                Ok::<_, mlua::Error>(table)
            };
            let cancellation = async { loop { if cancelled.load(Ordering::Acquire) { return; } tokio::time::sleep(Duration::from_millis(10)).await; } };
            let result = tokio::select! {
                result = tokio::time::timeout(remaining, io) => result.unwrap_or_else(|_| Err(mlua::Error::external("shell deadline exceeded"))),
                _ = cancellation => Err(mlua::Error::external("cancelled")),
            };
            if result.is_err() { let _ = child.start_kill(); let _ = child.wait().await; }
            result
        }
    }).map_err(|e| error(ToolErrorKind::ExecutionFailed, e.to_string()))?;
    globals.set("shell", shell.clone()).unwrap();
    let os_shell = shell.clone();
    os.set(
        "execute",
        lua.create_async_function(move |_, command: Option<String>| {
            let shell = os_shell.clone();
            async move {
                let Some(command) = command else {
                    return Ok((Some(true), None::<String>, None::<i32>));
                };
                let result: mlua::Table = shell.call_async(command).await?;
                if let Some(signal) = result.get::<Option<i32>>("signal")? {
                    return Ok((None, Some("signal".to_owned()), Some(signal)));
                }
                let code: i32 = result.get("exit_code")?;
                Ok((
                    if code == 0 { Some(true) } else { None },
                    Some("exit".to_owned()),
                    Some(code),
                ))
            }
        })
        .unwrap(),
    )
    .unwrap();
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
    let entry = lua.create_table().unwrap();
    entry.set("version", 1).unwrap();
    entry.set("context", context.clone()).unwrap();
    entry.set("arguments", arguments.clone()).unwrap();
    entry.set("json", json).unwrap();
    entry.set("shell", shell).unwrap();
    lua.set_named_registry_value("margatroid_entry", entry)
        .unwrap();
    globals
        .set(
            "margatroid",
            lua.create_function(|lua, ()| {
                lua.named_registry_value::<mlua::Table>("margatroid_entry")
            })
            .unwrap(),
        )
        .unwrap();
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

#[doc(hidden)]
pub async fn runner_main() -> i32 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args != [std::ffi::OsString::from("--margatroid-tool-worker")] {
        return crate::supervisor::supervisor_main(args).await;
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
    let response = RunnerResponse {
        protocol_version: 1,
        result,
        cleanup_proven: false,
    };
    let bytes = serde_json::to_vec(&response).unwrap();
    let mut stdout = tokio::io::stdout();
    let result = async {
        stdout.write_all(&bytes).await?;
        stdout.flush().await
    }
    .await;
    signal.abort();
    if result.is_ok() {
        0
    } else {
        3
    }
}
