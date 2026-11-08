//! Kernel-enforced aggregate memory accounting for a worker and its descendants.
use std::path::{Path, PathBuf};
use std::time::Duration;

use tool_core::{ToolError, ToolErrorKind as Kind};

use super::error;

mod global;
pub(crate) use global::MemoryPool;
use global::{GlobalGroup, Lease};
use std::sync::Arc;

#[derive(Clone)]
enum BoundaryKind {
    Cgroup(PathBuf),
    UserScope(String),
}
#[derive(Clone)]
pub(crate) struct MemoryBoundary {
    kind: BoundaryKind,
    parent: GlobalGroup,
    total: usize,
    lease: Option<Arc<Lease>>,
}

impl MemoryBoundary {
    pub(crate) async fn create(
        pool: &Arc<MemoryPool>,
        bytes: usize,
        total: usize,
    ) -> Result<Self, ToolError> {
        let (parent, lease) = pool.acquire(total).await?;
        let kind = match &parent {
            GlobalGroup::Slice(unit, _) => Ok(BoundaryKind::UserScope(format!(
                "{}-{}.scope",
                unit.trim_end_matches(".slice"),
                global::unique_name()
            ))),
            GlobalGroup::Cgroup(root) => {
                let path = root.join(global::unique_name());
                let setup = async {
                    tokio::fs::create_dir(&path).await.map_err(|e| {
                        error(
                            Kind::SandboxUnavailable,
                            format!("task memory cgroup creation failed: {e}"),
                        )
                    })?;
                    for (file, value) in [
                        ("memory.max", bytes.to_string()),
                        ("memory.swap.max", "0".into()),
                        ("memory.oom.group", "1".into()),
                    ] {
                        tokio::fs::write(path.join(file), value)
                            .await
                            .map_err(|e| {
                                error(
                                    Kind::SandboxUnavailable,
                                    format!("cannot enforce task {file}: {e}"),
                                )
                            })?;
                    }
                    global::verify_group(&path, bytes, "1").map_err(|e| {
                        error(
                            Kind::SandboxUnavailable,
                            format!("invalid task memory boundary: {e}"),
                        )
                    })?;
                    Ok::<_, ToolError>(BoundaryKind::Cgroup(path.clone()))
                }
                .await;
                if setup.is_err() {
                    let _ = tokio::fs::remove_dir(&path).await;
                }
                setup
            }
        };
        match kind {
            Ok(kind) => Ok(Self {
                kind,
                parent,
                total,
                lease: Some(lease),
            }),
            Err(failure) => {
                lease.release().await?;
                Err(failure)
            }
        }
    }

    pub(crate) fn export(&self, command: &mut tokio::process::Command) {
        command
            .env("MARGATROID_RUNNER_GLOBAL_CGROUP", self.parent.path())
            .env(
                "MARGATROID_RUNNER_TOTAL_MEMORY_BYTES",
                self.total.to_string(),
            );
        if let GlobalGroup::Slice(unit, _) = &self.parent {
            command.env("MARGATROID_RUNNER_GLOBAL_SLICE", unit);
        }
        match &self.kind {
            BoundaryKind::Cgroup(path) => {
                command.env("MARGATROID_RUNNER_CGROUP", path);
            }
            BoundaryKind::UserScope(unit) => {
                command.env("MARGATROID_RUNNER_SCOPE", unit);
            }
        }
    }

    pub(crate) fn inherited() -> Result<Option<Self>, ToolError> {
        let cgroup = std::env::var_os("MARGATROID_RUNNER_CGROUP");
        let scope = std::env::var("MARGATROID_RUNNER_SCOPE").ok();
        if cgroup.is_none() && scope.is_none() {
            return Ok(None);
        }
        let total = std::env::var("MARGATROID_RUNNER_TOTAL_MEMORY_BYTES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                error(
                    Kind::SandboxUnavailable,
                    "inherited total memory cap missing",
                )
            })?;
        let parent_path = std::env::var_os("MARGATROID_RUNNER_GLOBAL_CGROUP")
            .map(PathBuf::from)
            .ok_or_else(|| {
                error(
                    Kind::SandboxUnavailable,
                    "inherited global memory parent missing",
                )
            })?;
        let (kind, parent) = match (cgroup, scope) {
            (Some(path), None) => (
                BoundaryKind::Cgroup(PathBuf::from(path)),
                GlobalGroup::Cgroup(parent_path),
            ),
            (None, Some(unit)) => {
                let slice = std::env::var("MARGATROID_RUNNER_GLOBAL_SLICE").map_err(|_| {
                    error(Kind::SandboxUnavailable, "inherited memory slice missing")
                })?;
                if parent_path.file_name().and_then(|s| s.to_str()) != Some(slice.as_str()) {
                    return Err(error(
                        Kind::SandboxUnavailable,
                        "inherited memory slice path mismatch",
                    ));
                }
                (
                    BoundaryKind::UserScope(unit),
                    GlobalGroup::Slice(slice, parent_path),
                )
            }
            _ => {
                return Err(error(
                    Kind::SandboxUnavailable,
                    "ambiguous inherited memory boundary",
                ))
            }
        };
        global::verify_group(parent.path(), total, "0").map_err(|e| {
            error(
                Kind::SandboxUnavailable,
                format!("invalid inherited global memory parent: {e}"),
            )
        })?;
        if let BoundaryKind::Cgroup(path) = &kind {
            if path.parent() != Some(parent.path()) {
                return Err(error(
                    Kind::SandboxUnavailable,
                    "task cgroup escapes global memory parent",
                ));
            }
        }
        Ok(Some(Self {
            kind,
            parent,
            total,
            lease: None,
        }))
    }

    pub(crate) fn validate(&self, bytes: usize, total: usize) -> Result<(), ToolError> {
        if self.total != total {
            return Err(error(
                Kind::SandboxUnavailable,
                "inherited total memory limit does not match request",
            ));
        }
        global::verify_group(self.parent.path(), total, "0").map_err(|e| {
            error(
                Kind::SandboxUnavailable,
                format!("invalid global memory boundary: {e}"),
            )
        })?;
        if let BoundaryKind::Cgroup(path) = &self.kind {
            global::verify_group(path, bytes, "1").map_err(|e| {
                error(
                    Kind::SandboxUnavailable,
                    format!("invalid task memory boundary: {e}"),
                )
            })?;
        }
        Ok(())
    }

    pub(crate) fn command(
        &self,
        bytes: usize,
        executable: &Path,
        sandbox: Option<(&std::ffi::OsString, &std::ffi::OsString)>,
    ) -> tokio::process::Command {
        let mut command = match &self.kind {
            BoundaryKind::Cgroup(_) => tokio::process::Command::new(
                sandbox.map_or(executable.as_os_str(), |(backend, _)| backend.as_os_str()),
            ),
            BoundaryKind::UserScope(unit) => {
                let mut command = tokio::process::Command::new("systemd-run");
                command
                    .args(["--user", "--scope", "--quiet", "--unit"])
                    .arg(unit)
                    .arg(format!(
                        "--slice={}",
                        match &self.parent {
                            GlobalGroup::Slice(unit, _) => unit,
                            _ => unreachable!(),
                        }
                    ))
                    .arg(format!("--property=MemoryMax={bytes}"))
                    .args([
                        "--property=MemorySwapMax=0",
                        "--property=OOMPolicy=kill",
                        "--",
                    ])
                    .arg(executable)
                    .arg("--margatroid-tool-memory-entry")
                    .arg(bytes.to_string())
                    .arg(self.total.to_string())
                    .arg(self.parent.path())
                    .arg(
                        sandbox.map_or(executable.as_os_str(), |(backend, _)| backend.as_os_str()),
                    );
                command
            }
        };
        if let Some((_, policy)) = sandbox {
            command
                .arg("run")
                .arg("-p")
                .arg(policy)
                .arg("--")
                .arg(executable);
        }
        command
            .arg("--margatroid-tool-worker")
            .env_remove("MARGATROID_RUNNER_CGROUP")
            .env_remove("MARGATROID_RUNNER_SCOPE")
            .env_remove("MARGATROID_RUNNER_GLOBAL_CGROUP")
            .env_remove("MARGATROID_RUNNER_GLOBAL_SLICE")
            .env_remove("MARGATROID_RUNNER_TOTAL_MEMORY_BYTES");
        #[cfg(unix)]
        if let BoundaryKind::Cgroup(path) = &self.kind {
            let procs =
                std::ffi::CString::new(path.join("cgroup.procs").as_os_str().as_encoded_bytes())
                    .expect("cgroup paths cannot contain NUL");
            // Only async-signal-safe syscalls run after fork. Writing zero joins
            // the writing process, before either the sandbox or worker executes.
            unsafe {
                command.pre_exec(move || {
                    let fd =
                        nix::libc::open(procs.as_ptr(), nix::libc::O_WRONLY | nix::libc::O_CLOEXEC);
                    if fd == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let written = nix::libc::write(fd, b"0".as_ptr().cast(), 1);
                    let failure = if written == 1 {
                        None
                    } else {
                        Some(std::io::Error::last_os_error())
                    };
                    nix::libc::close(fd);
                    match failure {
                        Some(failure) => Err(failure),
                        None => Ok(()),
                    }
                });
            }
        }
        command
    }

    pub(crate) async fn oom_killed(&self) -> bool {
        match &self.kind {
            BoundaryKind::Cgroup(path) => tokio::fs::read_to_string(path.join("memory.events"))
                .await
                .is_ok_and(|s| {
                    s.lines()
                        .any(|line| line.strip_prefix("oom_kill ").is_some_and(|n| n != "0"))
                }),
            BoundaryKind::UserScope(unit) => tokio::process::Command::new("systemctl")
                .args(["--user", "show", "--property=Result", "--value", unit])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await
                .is_ok_and(|output| {
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout).trim() == "oom-kill"
                }),
        }
    }

    pub(crate) async fn kill(&self) -> Result<(), ToolError> {
        match &self.kind {
            BoundaryKind::Cgroup(path) => {
                if !path.exists() {
                    return Ok(());
                }
                tokio::fs::write(path.join("cgroup.kill"), "1")
                    .await
                    .map_err(|e| {
                        error(Kind::CleanupFailed, format!("task cgroup kill failed: {e}"))
                    })
            }
            BoundaryKind::UserScope(unit) => {
                let output = tokio::process::Command::new("systemctl")
                    .args(["--user", "kill", "--kill-whom=all", "--signal=KILL", unit])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| {
                        error(
                            Kind::CleanupFailed,
                            format!("task scope cleanup unavailable: {e}"),
                        )
                    })?;
                if output.status.success()
                    || String::from_utf8_lossy(&output.stderr).contains("not loaded")
                {
                    Ok(())
                } else {
                    let state = tokio::process::Command::new("systemctl")
                        .args(["--user", "show", "--property=ActiveState", "--value", unit])
                        .stdin(std::process::Stdio::null())
                        .output()
                        .await;
                    if state.is_ok_and(|o| {
                        matches!(
                            String::from_utf8_lossy(&o.stdout).trim(),
                            "inactive" | "failed"
                        )
                    }) {
                        Ok(())
                    } else {
                        Err(error(Kind::CleanupFailed, "task scope kill failed"))
                    }
                }
            }
        }
    }

    pub(crate) async fn cleanup(&self) -> Result<(), ToolError> {
        self.cleanup_task().await?;
        if let Some(lease) = &self.lease {
            lease.release().await?;
        }
        Ok(())
    }

    async fn cleanup_task(&self) -> Result<(), ToolError> {
        self.kill().await?;
        match &self.kind {
            BoundaryKind::Cgroup(path) => {
                if !path.exists() {
                    return Ok(());
                }
                loop {
                    let events = tokio::fs::read_to_string(path.join("cgroup.events"))
                        .await
                        .map_err(|e| {
                            error(
                                Kind::CleanupFailed,
                                format!("task cgroup status unavailable: {e}"),
                            )
                        })?;
                    if events.lines().any(|line| line == "populated 0") {
                        return tokio::fs::remove_dir(path).await.map_err(|e| {
                            error(
                                Kind::CleanupFailed,
                                format!("task cgroup removal failed: {e}"),
                            )
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
            BoundaryKind::UserScope(unit) => {
                let output = tokio::process::Command::new("systemctl")
                    .args(["--user", "stop", unit])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| {
                        error(
                            Kind::CleanupFailed,
                            format!("task scope removal unavailable: {e}"),
                        )
                    })?;
                if !output.status.success()
                    && !String::from_utf8_lossy(&output.stderr).contains("not loaded")
                {
                    return Err(error(Kind::CleanupFailed, "task scope removal failed"));
                }
                // Stopping an OOM-failed transient scope leaves its failed unit
                // loaded. Release only this task's record after reading Result.
                let reset = tokio::process::Command::new("systemctl")
                    .args(["--user", "reset-failed", unit])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| {
                        error(
                            Kind::CleanupFailed,
                            format!("task scope release unavailable: {e}"),
                        )
                    })?;
                if !reset.status.success()
                    && !String::from_utf8_lossy(&reset.stderr).contains("not loaded")
                {
                    return Err(error(Kind::CleanupFailed, "task scope release failed"));
                }
                Ok(())
            }
        }
    }
}

/// Trusted bootstrap: verify that the manager actually installed the controller
/// before executing a sandbox launcher or worker. Never silently run unrestricted.
#[cfg(target_os = "linux")]
pub(crate) fn enter_scope(args: &[std::ffi::OsString]) -> i32 {
    use std::os::unix::process::CommandExt;
    let Some((limit, command)) = args.split_first() else {
        return 2;
    };
    let Some(bytes) = limit.to_str().and_then(|s| s.parse::<u64>().ok()) else {
        return 2;
    };
    let [total, parent, program, arguments @ ..] = command else {
        return 2;
    };
    let Some(total) = total
        .to_str()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
    else {
        return 2;
    };
    let parent = Path::new(parent);
    let verify = || -> std::io::Result<()> {
        let membership = std::fs::read_to_string("/proc/self/cgroup")?;
        let relative = membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or_else(|| std::io::Error::other("cgroup v2 membership unavailable"))?;
        let path = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        if path.parent() != Some(parent) {
            return Err(std::io::Error::other(
                "task escaped the shared global memory parent",
            ));
        }
        global::verify_group(parent, total, "0")?;
        global::verify_group(&path, bytes as usize, "1")?;
        Ok(())
    };
    if let Err(failure) = verify() {
        eprintln!("task aggregate memory enforcement unavailable: {failure}");
        return 3;
    }
    let failure = std::process::Command::new(program).args(arguments).exec();
    eprintln!("memory-enforced worker launch failed: {failure}");
    3
}
