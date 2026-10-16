use crate::{RunnerRequest, RunnerResponse, ToolError, ToolErrorKind};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn error(kind: ToolErrorKind, message: &str) -> ToolError {
    ToolError::new(kind, message)
}

#[cfg(unix)]
async fn shutdown_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::select! {
        _ = term.recv() => {},
        _ = interrupt.recv() => {},
        _ = hangup.recv() => {},
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) async fn supervisor_main(args: Vec<std::ffi::OsString>) -> i32 {
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
            std::path::PathBuf::from(path).join("cgroup.procs"),
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
    let mut raw = Vec::new();
    let mut stdin = tokio::io::stdin().take(4 * 1024 * 1024 + 1);
    let input = stdin.read_to_end(&mut raw);
    let input = tokio::select! {
        result = input => result,
        _ = &mut shutdown => return 3,
    };
    let request = if input.is_ok() && raw.len() <= 4 * 1024 * 1024 {
        serde_json::from_slice::<RunnerRequest>(&raw).ok()
    } else {
        None
    };
    let Some(request) = request else {
        return 2;
    };
    if request.protocol_version != 1 || request.limits.validate().is_err() {
        return 2;
    }
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
    let result = match command.spawn() {
        Err(_) => Err(error(
            ToolErrorKind::RunnerStartFailed,
            "worker could not start",
        )),
        Ok(mut worker) => {
            let mut stdin = worker.stdin.take().unwrap();
            let stdout = worker.stdout.take().unwrap();
            let input = async move {
                stdin.write_all(&raw).await?;
                stdin.shutdown().await
            };
            let limit = request
                .limits
                .result_bytes
                .saturating_add(request.limits.output_bytes.saturating_mul(6))
                .saturating_add(4096);
            let output = async {
                let mut bytes = Vec::new();
                stdout
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                Ok::<_, std::io::Error>(bytes)
            };
            let mut output_bytes = None;
            tokio::pin!(input, output);
            let mut input_done = false;
            let mut output_done = false;
            let deadline = tokio::time::sleep(request.limits.execution_timeout);
            tokio::pin!(deadline);
            let outcome = loop {
                tokio::select! {
                    value = &mut input, if !input_done => {
                        input_done = true;
                        if value.is_err() { break Err(error(ToolErrorKind::RunnerProtocolFailed, "worker input failed")); }
                    },
                    value = &mut output, if !output_done => {
                        output_done = true;
                        match value {
                            Ok(bytes) => output_bytes = Some(bytes),
                            Err(_) => break Err(error(ToolErrorKind::RunnerProtocolFailed, "worker output failed")),
                        }
                    },
                    value = worker.wait() => break value.map_err(|_| error(ToolErrorKind::CleanupFailed, "worker wait failed")).and_then(|status| {
                        if status.success() { Ok(status) } else { Err(error(ToolErrorKind::ExecutionFailed, "worker exited unsuccessfully")) }
                    }),
                    _ = &mut shutdown => break Err(error(ToolErrorKind::Cancelled, "supervisor shutdown requested")),
                    _ = &mut deadline => break Err(error(ToolErrorKind::Timeout, "worker deadline exceeded")),
                }
            };
            let proven = cleanup(request.limits.cleanup_timeout).await;
            let _ = tokio::time::timeout(request.limits.cleanup_timeout, worker.wait()).await;
            if !proven {
                Err(error(
                    ToolErrorKind::CleanupFailed,
                    "supervisor descendant cleanup failed",
                ))
            } else if let Err(failure) = outcome {
                Err(failure)
            } else {
                if !output_done {
                    output_bytes =
                        tokio::time::timeout(request.limits.cleanup_timeout, &mut output)
                            .await
                            .ok()
                            .and_then(Result::ok);
                }
                let bytes = output_bytes.unwrap_or_default();
                if bytes.is_empty() || bytes.len() > limit {
                    Err(error(
                        ToolErrorKind::RunnerProtocolFailed,
                        "worker response exceeds limit or remains open",
                    ))
                } else {
                    serde_json::from_slice::<RunnerResponse>(&bytes)
                        .map_err(|_| {
                            error(
                                ToolErrorKind::RunnerProtocolFailed,
                                "invalid worker response",
                            )
                        })
                        .and_then(|response| {
                            if response.protocol_version != 1 || response.cleanup_proven {
                                Err(error(
                                    ToolErrorKind::RunnerProtocolFailed,
                                    "invalid worker protocol",
                                ))
                            } else {
                                response.result
                            }
                        })
                }
            }
        }
    };
    let cleanup_proven = cleanup(request.limits.cleanup_timeout).await;
    let response = RunnerResponse {
        protocol_version: 1,
        result: if cleanup_proven {
            result
        } else {
            Err(error(
                ToolErrorKind::CleanupFailed,
                "supervisor descendant cleanup failed",
            ))
        },
        cleanup_proven,
    };
    let Ok(bytes) = serde_json::to_vec(&response) else {
        return 3;
    };
    let mut stdout = tokio::io::stdout();
    let written = async {
        stdout.write_all(&bytes).await?;
        stdout.flush().await
    }
    .await;
    if written.is_ok() {
        0
    } else {
        3
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn supervisor_main(_: Vec<std::ffi::OsString>) -> i32 {
    3
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
                continue
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
async fn cleanup(timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
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
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
