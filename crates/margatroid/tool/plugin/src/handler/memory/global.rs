//! One lazily allocated parent for all active tasks of a ToolPlugin instance.
use super::super::error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tool_core::{ToolError, ToolErrorKind as Kind};

static NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) fn unique_name() -> String {
    format!(
        "margatroidtools{}n{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
#[derive(Clone)]
pub(super) enum GlobalGroup {
    Cgroup(PathBuf),
    Slice(String, PathBuf),
}
#[derive(Default)]
pub(crate) struct MemoryPool(tokio::sync::Mutex<Option<ActiveGroup>>);
struct ActiveGroup {
    group: GlobalOwner,
    bytes: usize,
    users: usize,
}
pub(super) struct Lease {
    pool: Arc<MemoryPool>,
    released: AtomicBool,
}
impl MemoryPool {
    pub(super) async fn acquire(
        self: &Arc<Self>,
        bytes: usize,
    ) -> Result<(GlobalGroup, Arc<Lease>), ToolError> {
        let mut state = self.0.lock().await;
        if state.is_none() {
            *state = Some(ActiveGroup {
                group: GlobalGroup::create(bytes).await?,
                bytes,
                users: 0,
            });
        }
        let active = state.as_mut().unwrap();
        if active.bytes != bytes {
            return Err(error(
                Kind::SandboxUnavailable,
                "shared memory parent limit changed while tasks are active",
            ));
        }
        active.users += 1;
        Ok((
            active.group.group.clone(),
            Arc::new(Lease {
                pool: self.clone(),
                released: AtomicBool::new(false),
            }),
        ))
    }
}
impl Lease {
    pub(super) async fn release(&self) -> Result<(), ToolError> {
        let mut state = self.pool.0.lock().await;
        if self.released.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(active) = state.as_mut() {
            if active.users == 1 {
                active.group.group.cleanup().await?;
                active.group.armed = false;
                *state = None;
            } else {
                active.users -= 1;
            }
        }
        self.released.store(true, Ordering::Release);
        Ok(())
    }
}
// Own cleanup independently of task leases, including while creation is pending.
// This synchronous fallback runs only if the runtime drops an unfinished future.
struct GlobalOwner {
    group: GlobalGroup,
    armed: bool,
}
impl GlobalOwner {
    fn new(group: GlobalGroup) -> Self {
        Self { group, armed: true }
    }
}
impl Drop for GlobalOwner {
    fn drop(&mut self) {
        if self.armed {
            if let Err(failure) = self.group.cleanup_sync() {
                eprintln!("global memory parent drop cleanup failed: {failure}");
            }
        }
    }
}
impl GlobalGroup {
    async fn create(bytes: usize) -> Result<GlobalOwner, ToolError> {
        if !cfg!(target_os = "linux") || bytes == 0 {
            return Err(error(
                Kind::SandboxUnavailable,
                "aggregate tool memory limits require Linux cgroup v2 and a positive limit",
            ));
        }
        let name = unique_name();
        if let Some(root) = std::env::var_os("MARGATROID_TOOL_CGROUP_ROOT") {
            let root = PathBuf::from(root);
            if !root.is_absolute() || !root.join("cgroup.controllers").is_file() {
                return Err(error(
                    Kind::SandboxUnavailable,
                    "configured cgroup v2 root unavailable",
                ));
            }
            #[cfg(target_os = "linux")]
            {
                let root_c = std::ffi::CString::new(root.as_os_str().as_encoded_bytes())
                    .map_err(|_| error(Kind::SandboxUnavailable, "invalid cgroup root path"))?;
                let mut stat = std::mem::MaybeUninit::<nix::libc::statfs>::uninit();
                let valid = unsafe {
                    nix::libc::statfs(root_c.as_ptr(), stat.as_mut_ptr()) == 0
                        && stat.assume_init().f_type == nix::libc::CGROUP2_SUPER_MAGIC
                };
                if !valid {
                    return Err(error(
                        Kind::SandboxUnavailable,
                        "configured memory root is not a cgroup v2 filesystem",
                    ));
                }
            }
            let controllers = tokio::fs::read_to_string(root.join("cgroup.subtree_control"))
                .await
                .map_err(|e| {
                    error(
                        Kind::SandboxUnavailable,
                        format!("cannot inspect delegated controller: {e}"),
                    )
                })?;
            if !controllers.split_whitespace().any(|c| c == "memory") {
                return Err(error(
                    Kind::SandboxUnavailable,
                    "configured cgroup root must enable memory controller",
                ));
            }
            let path = root.join(name);
            std::fs::create_dir(&path).map_err(|e| {
                error(
                    Kind::SandboxUnavailable,
                    format!("global memory parent creation failed: {e}"),
                )
            })?;
            let owner = GlobalOwner::new(Self::Cgroup(path.clone()));
            let setup = async {
                for (file, value) in [
                    ("memory.max", bytes.to_string()),
                    ("memory.swap.max", "0".into()),
                    ("memory.oom.group", "0".into()),
                    ("cgroup.subtree_control", "+memory".into()),
                ] {
                    tokio::fs::write(path.join(file), value)
                        .await
                        .map_err(|e| {
                            error(
                                Kind::SandboxUnavailable,
                                format!("cannot enforce global {file}: {e}"),
                            )
                        })?;
                }
                Ok::<_, ToolError>(())
            }
            .await;
            if let Err(failure) = setup {
                let _ = tokio::fs::remove_dir(&path).await;
                return Err(failure);
            }
            if let Err(failure) = verify_group(&path, bytes, "0") {
                let _ = tokio::fs::remove_dir(&path).await;
                return Err(error(
                    Kind::SandboxUnavailable,
                    format!("global memory parent enforcement invalid: {failure}"),
                ));
            }
            return Ok(owner);
        }
        let unit = format!("{name}.slice");
        let mut owner = GlobalOwner::new(Self::Slice(unit.clone(), PathBuf::new()));
        let output = tokio::process::Command::new("busctl")
            .kill_on_drop(true)
            .args([
                "--user",
                "call",
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "StartTransientUnit",
                "ssa(sv)a(sa(sv))",
                &unit,
                "fail",
                "3",
                "MemoryMax",
                "t",
                &bytes.to_string(),
                "MemorySwapMax",
                "t",
                "0",
                "CollectMode",
                "s",
                "inactive-or-failed",
                "0",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| {
                error(
                    Kind::SandboxUnavailable,
                    format!("global systemd memory slice unavailable: {e}"),
                )
            })?;
        if !output.status.success() {
            return Err(error(
                Kind::SandboxUnavailable,
                format!(
                    "global memory slice creation failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        // StartTransientUnit returns a queued job, before ControlGroup exists.
        // A synchronous start merges with that job and waits for completion.
        let started = tokio::process::Command::new("systemctl")
            .args(["--user", "start", &unit])
            .stdin(std::process::Stdio::null())
            .output()
            .await;
        if !started.is_ok_and(|output| output.status.success()) {
            let temporary = Self::Slice(unit, PathBuf::new());
            temporary.cleanup().await?;
            return Err(error(
                Kind::SandboxUnavailable,
                "global memory slice start job failed",
            ));
        }
        let inspect = tokio::process::Command::new("systemctl")
            .args([
                "--user",
                "show",
                "--property=ControlGroup",
                "--value",
                &unit,
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .await;
        let inspected = match inspect {
            Ok(output) if output.status.success() => {
                let relative = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if !relative.starts_with('/') || relative == "/" {
                    Err(std::io::Error::other("slice control group unavailable"))
                } else {
                    let path =
                        PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
                    verify_group(&path, bytes, "0").map(|()| path)
                }
            }
            _ => Err(std::io::Error::other(
                "slice control group inspection failed",
            )),
        };
        match inspected {
            Ok(path) => {
                owner.group = Self::Slice(unit, path);
                Ok(owner)
            }
            Err(failure) => {
                let temporary = Self::Slice(unit, PathBuf::new());
                temporary.cleanup().await?;
                Err(error(
                    Kind::SandboxUnavailable,
                    format!("global memory enforcement invalid: {failure}"),
                ))
            }
        }
    }
    pub(super) fn path(&self) -> &std::path::Path {
        match self {
            Self::Cgroup(path) | Self::Slice(_, path) => path,
        }
    }
    fn cleanup_sync(&self) -> std::io::Result<()> {
        match self {
            Self::Slice(unit, _) => {
                let prefix = format!("{}-", unit.trim_end_matches(".slice"));
                let listed = std::process::Command::new("systemctl")
                    .args([
                        "--user",
                        "list-units",
                        "--all",
                        "--plain",
                        "--no-legend",
                        "--no-pager",
                        &format!("{prefix}*.scope"),
                    ])
                    .stdin(std::process::Stdio::null())
                    .output()?;
                if !listed.status.success() {
                    return Err(std::io::Error::other("owned task scope inspection failed"));
                }
                let children: Vec<String> = String::from_utf8_lossy(&listed.stdout)
                    .lines()
                    .filter_map(|line| line.split_whitespace().next())
                    .filter(|name| name.starts_with(&prefix) && name.ends_with(".scope"))
                    .map(str::to_owned)
                    .collect();
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", unit])
                    .stdin(std::process::Stdio::null())
                    .output()?;
                if !output.status.success()
                    && !String::from_utf8_lossy(&output.stderr).contains("not loaded")
                {
                    return Err(std::io::Error::other("global memory slice stop failed"));
                }
                for child in children {
                    let reset = std::process::Command::new("systemctl")
                        .args(["--user", "reset-failed", &child])
                        .stdin(std::process::Stdio::null())
                        .output()?;
                    if !reset.status.success()
                        && !String::from_utf8_lossy(&reset.stderr).contains("not loaded")
                    {
                        return Err(std::io::Error::other("owned task scope release failed"));
                    }
                }
                Ok(())
            }
            Self::Cgroup(path) => {
                if !path.exists() {
                    return Ok(());
                }
                // The last owner means every boundary has been dropped. Kill
                // only this instance's hierarchy before removing its directories.
                std::fs::write(path.join("cgroup.kill"), "1")?;
                fn remove_empty(path: &std::path::Path) -> std::io::Result<()> {
                    for entry in std::fs::read_dir(path)? {
                        let entry = entry?;
                        if entry.file_type()?.is_dir() {
                            remove_empty(&entry.path())?;
                        }
                    }
                    loop {
                        let events = std::fs::read_to_string(path.join("cgroup.events"))?;
                        if events.lines().any(|line| line == "populated 0") {
                            return std::fs::remove_dir(path);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                remove_empty(path)
            }
        }
    }
    async fn cleanup(&self) -> Result<(), ToolError> {
        match self {
            Self::Cgroup(path) => match tokio::fs::remove_dir(path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(error(
                    Kind::CleanupFailed,
                    format!("global memory parent removal failed: {e}"),
                )),
            },
            Self::Slice(unit, _) => {
                let output = tokio::process::Command::new("systemctl")
                    .args(["--user", "stop", unit])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|e| {
                        error(
                            Kind::CleanupFailed,
                            format!("global memory slice cleanup unavailable: {e}"),
                        )
                    })?;
                if output.status.success()
                    || String::from_utf8_lossy(&output.stderr).contains("not loaded")
                {
                    Ok(())
                } else {
                    Err(error(
                        Kind::CleanupFailed,
                        "global memory slice cleanup failed",
                    ))
                }
            }
        }
    }
}

/// Read kernel controller files rather than trusting manager properties.
pub(super) fn verify_group(
    path: &std::path::Path,
    bytes: usize,
    oom_group: &str,
) -> std::io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(std::io::Error::other("invalid memory control group path"));
    }
    #[cfg(target_os = "linux")]
    {
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| std::io::Error::other("invalid memory group path"))?;
        let mut stat = std::mem::MaybeUninit::<nix::libc::statfs>::uninit();
        let valid = unsafe {
            nix::libc::statfs(cpath.as_ptr(), stat.as_mut_ptr()) == 0
                && stat.assume_init().f_type == nix::libc::CGROUP2_SUPER_MAGIC
        };
        if !valid {
            return Err(std::io::Error::other("memory group is not on cgroup v2"));
        }
    }
    let cap = std::fs::read_to_string(path.join("memory.max"))?
        .trim()
        .parse::<usize>()
        .map_err(|_| std::io::Error::other("memory.max is unlimited"))?;
    if cap == 0
        || cap > bytes
        || std::fs::read_to_string(path.join("memory.swap.max"))?.trim() != "0"
        || std::fs::read_to_string(path.join("memory.oom.group"))?.trim() != oom_group
    {
        return Err(std::io::Error::other(
            "memory controller does not enforce requested limits",
        ));
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn assert_removed(group: &GlobalGroup) {
        assert!(
            !group.path().exists(),
            "global cgroup survived final owner drop: {}",
            group.path().display()
        );
        if let GlobalGroup::Slice(unit, _) = group {
            // stop waits for deactivation and cgroup removal; transient unit
            // garbage collection can follow in a later manager event-loop turn.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let output = std::process::Command::new("systemctl")
                    // show would load a new synthetic slice unit by name.
                    .args([
                        "--user",
                        "list-units",
                        "--all",
                        "--plain",
                        "--no-legend",
                        "--no-pager",
                        unit,
                    ])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                if output.stdout.is_empty() {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "stopped transient slice was not collected"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    #[tokio::test]
    async fn final_pool_owner_drop_removes_unreleased_parent() {
        let pool = Arc::new(MemoryPool::default());
        let (group, lease) = pool.acquire(64 * 1024 * 1024).await.unwrap();
        let (other_group, other_lease) = pool.acquire(64 * 1024 * 1024).await.unwrap();
        assert_eq!(group.path(), other_group.path());
        drop(lease);
        assert!(
            group.path().exists(),
            "dropping one lease stopped a shared parent"
        );
        drop(other_lease);
        assert!(group.path().exists(), "plugin pool must retain ownership");
        assert!(std::fs::read_to_string(group.path().join("cgroup.procs"))
            .unwrap()
            .trim()
            .is_empty());
        drop(pool);
        assert_removed(&group);
    }

    #[tokio::test]
    async fn aborted_task_releases_last_pool_owner_without_async_cleanup() {
        let pool = Arc::new(MemoryPool::default());
        let task_pool = pool.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (group, _lease) = task_pool.acquire(64 * 1024 * 1024).await.unwrap();
            sender
                .send(group)
                .unwrap_or_else(|_| panic!("test receiver dropped"));
            std::future::pending::<()>().await;
        });
        let group = receiver.await.unwrap();
        assert!(std::fs::read_to_string(group.path().join("cgroup.events"))
            .unwrap()
            .lines()
            .any(|line| line == "populated 0"));
        drop(pool);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_removed(&group);
    }
}
