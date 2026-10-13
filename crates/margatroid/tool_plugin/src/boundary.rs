use crate::{ToolError, ToolErrorKind};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) struct CgroupBoundary {
    path: PathBuf,
}
impl CgroupBoundary {
    pub(crate) async fn create() -> Result<Option<Self>, ToolError> {
        let Some(root) = std::env::var_os("MARGATROID_TOOL_CGROUP_ROOT") else {
            return Ok(None);
        };
        let root = PathBuf::from(root);
        if !root.is_absolute() || !root.join("cgroup.controllers").is_file() {
            return Err(ToolError::new(
                ToolErrorKind::SandboxUnavailable,
                "configured cgroup v2 root unavailable",
            ));
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = root.join(format!(
            "margatroid-tool-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        tokio::fs::create_dir(&path).await.map_err(|_| {
            ToolError::new(ToolErrorKind::SandboxUnavailable, "cgroup creation failed")
        })?;
        Ok(Some(Self { path }))
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub(crate) async fn cleanup(&self, timeout: Duration) -> Result<(), ToolError> {
        tokio::fs::write(self.path.join("cgroup.kill"), "1")
            .await
            .map_err(|_| ToolError::new(ToolErrorKind::CleanupFailed, "cgroup kill failed"))?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let events = tokio::fs::read_to_string(self.path.join("cgroup.events"))
                .await
                .map_err(|_| {
                    ToolError::new(ToolErrorKind::CleanupFailed, "cgroup status unavailable")
                })?;
            if events.lines().any(|line| line == "populated 0") {
                return tokio::fs::remove_dir(&self.path).await.map_err(|_| {
                    ToolError::new(ToolErrorKind::CleanupFailed, "cgroup removal failed")
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ToolError::new(
                    ToolErrorKind::CleanupFailed,
                    "cgroup remains populated",
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
