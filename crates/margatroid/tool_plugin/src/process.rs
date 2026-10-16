use crate::{RunnerRequest, RunnerResponse, ToolError, ToolErrorKind, ToolExecuteReply};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

fn error(kind: ToolErrorKind, message: &str) -> ToolError {
    ToolError::new(kind, message)
}
fn backend() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MARGATROID_SANDBOX_BACKEND") {
        return Some(path.into());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("landstrip"))
            .find(|p| p.is_file())
    })
}
pub(crate) fn filtered_environment(command: &mut tokio::process::Command) {
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let name_upper = name.to_string_lossy().to_ascii_uppercase();
        if ["KEY", "PASSWORD", "SECRET", "TOKEN"]
            .iter()
            .any(|p| name_upper.contains(p))
        {
            continue;
        }
        if [
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "LUA_PATH",
            "LUA_CPATH",
            "BASH_ENV",
            "ENV",
        ]
        .contains(&name_upper.as_str())
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
async fn drain<R: AsyncRead + Unpin>(
    mut pipe: R,
    limit: usize,
    overflow: CancellationToken,
) -> Result<Vec<u8>, ToolError> {
    let mut bytes = Vec::new();
    let mut exceeded = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe
            .read(&mut chunk)
            .await
            .map_err(|_| error(ToolErrorKind::RunnerProtocolFailed, "pipe read failed"))?;
        if count == 0 {
            return if exceeded {
                Err(error(
                    ToolErrorKind::OutputTooLarge,
                    "runner pipe exceeds limit",
                ))
            } else {
                Ok(bytes)
            };
        }
        if bytes.len().saturating_add(count) > limit {
            exceeded = true;
            overflow.cancel();
        }
        if !exceeded {
            bytes.extend_from_slice(&chunk[..count]);
        }
    }
}
pub(crate) async fn closed(reply: &ToolExecuteReply) {
    loop {
        if reply
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|s| s.is_closed())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
pub(crate) async fn run_runner(
    runner: &Path,
    payload: RunnerRequest,
    policy: Option<&str>,
    cancel: &CancellationToken,
    request_cancel: &CancellationToken,
    turn_cancel: &CancellationToken,
    reply: &ToolExecuteReply,
) -> Result<Option<String>, ToolError> {
    let deadline = tokio::time::Instant::now() + payload.limits.execution_timeout;
    let mut command = tokio::process::Command::new(runner);
    command.arg("--margatroid-tool-supervisor");
    if let Some(policy) = policy {
        let backend = backend().ok_or_else(|| {
            error(
                ToolErrorKind::SandboxUnavailable,
                "sandbox policy active but backend unavailable",
            )
        })?;
        let path = payload.context.temp_dir.join("policy.json");
        if policy.len() > payload.limits.script_bytes {
            return Err(error(
                ToolErrorKind::InvalidDefinition,
                "sandbox policy exceeds staging limit",
            ));
        }
        tokio::time::timeout_at(deadline, tokio::fs::write(&path, policy))
            .await
            .map_err(|_| error(ToolErrorKind::Timeout, "policy staging timed out"))?
            .map_err(|_| error(ToolErrorKind::PreparationFailed, "policy staging failed"))?;
        command.arg("--sandbox").arg(backend).arg(path);
    }
    filtered_environment(&mut command);
    command
        .current_dir(&payload.context.project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let cgroup = crate::boundary::CgroupBoundary::create().await?;
    if let Some(boundary) = &cgroup {
        command.env("MARGATROID_RUNNER_CGROUP", boundary.path());
    }
    let raw = serde_json::to_vec(&payload).map_err(|_| {
        error(
            ToolErrorKind::InvalidRequest,
            "payload serialization failed",
        )
    })?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            if let Some(boundary) = &cgroup {
                boundary.cleanup(payload.limits.cleanup_timeout).await?;
            }
            return Err(error(
                ToolErrorKind::RunnerStartFailed,
                "runner could not start",
            ));
        }
    };
    let pid = child
        .id()
        .ok_or_else(|| error(ToolErrorKind::RunnerStartFailed, "runner has no process ID"))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let input = async move {
        stdin
            .write_all(&raw)
            .await
            .map_err(|_| error(ToolErrorKind::RunnerProtocolFailed, "runner stdin failed"))?;
        stdin.shutdown().await.map_err(|_| {
            error(
                ToolErrorKind::RunnerProtocolFailed,
                "runner stdin close failed",
            )
        })
    };
    let overflow = CancellationToken::new();
    let out = drain(
        stdout,
        payload
            .limits
            .output_bytes
            .saturating_mul(6)
            .saturating_add(4096),
        overflow.clone(),
    );
    let err = drain(stderr, payload.limits.diagnostic_bytes, overflow.clone());
    tokio::pin!(input, out, err);
    let mut input_result = None;
    let mut output_result = None;
    let mut diagnostic_result = None;
    let mut status = None;
    let mut turn_aborted = false;
    let failure = loop {
        if input_result.is_some()
            && output_result.is_some()
            && diagnostic_result.is_some()
            && status.is_some()
        {
            break None;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break Some(error(ToolErrorKind::Cancelled, "scope cancelled")),
            _ = request_cancel.cancelled() => break Some(error(ToolErrorKind::Cancelled, "request cancelled")),
            value = &mut input, if input_result.is_none() => { let failure = value.as_ref().err().cloned(); input_result = Some(value); if failure.is_some() { break failure; } },
            value = &mut out, if output_result.is_none() => { let failure = value.as_ref().err().cloned(); output_result = Some(value); if failure.is_some() { break failure; } },
            value = &mut err, if diagnostic_result.is_none() => { let failure = value.as_ref().err().cloned(); diagnostic_result = Some(value); if failure.is_some() { break failure; } },
            _ = overflow.cancelled() => break Some(error(ToolErrorKind::OutputTooLarge, "runner pipe exceeds limit")),
            value = child.wait(), if status.is_none() => { status = Some(value.map_err(|_| error(ToolErrorKind::CleanupFailed, "runner wait failed"))?); },
            _ = turn_cancel.cancelled() => {
                turn_aborted = true;
                break Some(error(ToolErrorKind::Cancelled, "turn aborted"));
            },
            _ = closed(reply) => break Some(error(ToolErrorKind::Cancelled, "receiver closed")),
            _ = tokio::time::sleep_until(deadline) => break Some(error(ToolErrorKind::Timeout, "execution deadline exceeded")),
        }
    };
    if let Some(failure) = failure {
        #[cfg(unix)]
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
        let cleanup =
            async {
                if output_result.is_none() {
                    output_result = Some((&mut out).await);
                }
                if diagnostic_result.is_none() {
                    diagnostic_result = Some((&mut err).await);
                }
                if status.is_none() {
                    status =
                        Some(child.wait().await.map_err(|_| {
                            error(ToolErrorKind::CleanupFailed, "runner wait failed")
                        })?);
                }
                Ok::<(), ToolError>(())
            };
        let cleanup_result = tokio::time::timeout(payload.limits.cleanup_timeout, cleanup).await;
        if let Ok(Err(error)) = &cleanup_result {
            return Err(error.clone());
        }
        if cleanup_result.is_err() {
            #[cfg(unix)]
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = child.start_kill();
            let _ = tokio::time::timeout(payload.limits.cleanup_timeout, child.wait()).await;
            if let Some(boundary) = &cgroup {
                boundary.cleanup(payload.limits.cleanup_timeout).await?;
                if status.is_none() {
                    let _ =
                        tokio::time::timeout(payload.limits.cleanup_timeout, child.wait()).await;
                }
            }
            return Err(error(
                ToolErrorKind::CleanupFailed,
                "runner required forced termination; pipe cleanup unproven",
            ));
        }
        if let Some(boundary) = &cgroup {
            boundary.cleanup(payload.limits.cleanup_timeout).await?;
        }
        let response = output_result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|b| serde_json::from_slice::<RunnerResponse>(b).ok());
        if !response.is_some_and(|r| r.protocol_version == 1 && r.cleanup_proven) {
            return Err(error(
                ToolErrorKind::CleanupFailed,
                "runner did not prove descendant cleanup",
            ));
        }
        return if turn_aborted && !cancel.is_cancelled() && !request_cancel.is_cancelled() {
            Ok(None)
        } else {
            Err(failure)
        };
    }
    if let Some(boundary) = &cgroup {
        boundary.cleanup(payload.limits.cleanup_timeout).await?;
    }
    let response: RunnerResponse =
        serde_json::from_slice(&output_result.unwrap()?).map_err(|_| {
            error(
                ToolErrorKind::CleanupFailed,
                "invalid runner protocol; descendant cleanup unproven",
            )
        })?;
    if response.protocol_version != 1 || !response.cleanup_proven {
        return Err(error(
            ToolErrorKind::CleanupFailed,
            "runner cleanup proof missing",
        ));
    }
    if !status.unwrap().success() {
        return Err(error(
            ToolErrorKind::ExecutionFailed,
            "runner exited unsuccessfully",
        ));
    }
    let output = response.result?;
    if output.len() > payload.limits.output_bytes {
        return Err(error(
            ToolErrorKind::OutputTooLarge,
            "tool result exceeds limit",
        ));
    }
    Ok(Some(output))
}
