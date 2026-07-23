//! Margatroid 沙箱运行时
//!
//! OS 原生的进程级沙箱隔离，不依赖 Docker 或虚拟机。
//! Linux 使用 bubblewrap，macOS 使用 sandbox-exec。
//!
//! # 守卫机制
//!
//! `allow_unsandboxed_commands: false` 时，
//! `guard()` 拒绝任何未经 wrap_command() 包装的裸命令。

pub mod config;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod mandatory;

use anyhow::{Result, bail};
use config::SandboxConfig;
use std::future::Future;
use std::pin::Pin;

// ── Trait ────────────────────────────────────────────────────

pub trait Sandbox: Send + Sync {
    fn initialize(
        &mut self,
        config: SandboxConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// 将任意 shell 命令包装为沙箱化命令
    fn wrap_command(&self, cmd: &str) -> String;

    fn reset(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    fn platform(&self) -> &'static str;
}

// ── Manager ──────────────────────────────────────────────────

/// 沙箱管理器
///
/// 根据当前平台自动选择实现，提供守卫 + 命令包装 + 代理管理。
pub struct SandboxManager {
    inner: Box<dyn Sandbox>,
    config: Option<SandboxConfig>,
}

impl SandboxManager {
    pub fn new() -> Self {
        Self {
            inner: Self::create_platform(),
            config: None,
        }
    }

    fn create_platform() -> Box<dyn Sandbox> {
        #[cfg(target_os = "linux")]
        {
            Box::new(linux::LinuxSandbox::new())
        }
        #[cfg(target_os = "macos")]
        {
            Box::new(macos::MacOSSandbox::new())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            compile_error!("Margatroid sandbox only supports Linux and macOS");
        }
    }

    /// 初始化沙箱并存储配置（供 guard() 使用）
    pub async fn initialize(&mut self, config: SandboxConfig) -> Result<()> {
        self.inner.initialize(config.clone()).await?;
        self.config = Some(config);
        Ok(())
    }

    /// 将命令包装为沙箱化命令
    pub fn wrap_command(&self, cmd: &str) -> String {
        self.inner.wrap_command(cmd)
    }

    /// 沙箱守卫：如果 `allow_unsandboxed_commands` 为 false，
    /// 拒绝任何未经 wrap_command() 包装的裸命令。
    ///
    /// 被排除的命令（excluded_commands 列表中的）不受此限制。
    ///
    /// 返回 `Ok(true)` 表示命令已在沙箱内，根据 `auto_allow_bash_if_sandboxed`
    /// 配置可跳过权限弹窗。
    pub fn guard(&self, cmd: &str) -> Result<bool> {
        let config = match &self.config {
            Some(c) => c,
            None => return Ok(false),
        };

        if !config.enabled || config.allow_unsandboxed_commands {
            return Ok(false);
        }

        // 检查是否为排除命令
        for excluded in &config.excluded_commands {
            if cmd.starts_with(excluded) {
                return Ok(false);
            }
        }

        let is_sandboxed = cmd.contains("bwrap") || cmd.contains("sandbox-exec");

        if !is_sandboxed {
            bail!(
                "拒绝执行未经沙箱包装的命令: '{}'\n\
                 所有命令必须通过沙箱执行。使用 sandbox.wrap_command() 包装后再执行。",
                cmd
            );
        }

        // 如果是沙箱内命令且 auto_allow 开启，返回 true（调用方可跳过权限提示）
        Ok(config.auto_allow_bash_if_sandboxed)
    }

    /// 获取当前配置
    pub fn config(&self) -> Option<&SandboxConfig> {
        self.config.as_ref()
    }

    pub async fn reset(&mut self) -> Result<()> {
        self.inner.reset().await?;
        self.config = None;
        Ok(())
    }

    pub fn platform(&self) -> &'static str {
        self.inner.platform()
    }
}

impl Default for SandboxManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Config helpers ───────────────────────────────────────────

/// 加载用户级默认沙箱配置
///
/// 从 `~/.margatroid/sandbox.toml` 读取。
pub fn load_user_config() -> Result<SandboxConfig> {
    let path = paths::margatroid_root()
        .unwrap_or_else(|| std::path::PathBuf::from(".margatroid"))
        .join("sandbox.toml");

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let config: SandboxConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("sandbox.toml 解析失败: {}", e))?;
        Ok(config)
    } else {
        Ok(SandboxConfig::default())
    }
}

/// 保存用户级默认沙箱配置
pub fn save_user_config(config: &SandboxConfig) -> Result<()> {
    let path = paths::margatroid_root()
        .unwrap_or_else(|| std::path::PathBuf::from(".margatroid"))
        .join("sandbox.toml");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guard_rejects_unsandboxed_command() {
        let mut mgr = SandboxManager::new();
        mgr.initialize(SandboxConfig::strict()).await.unwrap();

        let result = mgr.guard("rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("拒绝"));
    }

    #[tokio::test]
    async fn guard_allows_wrapped_command() {
        let mut mgr = SandboxManager::new();
        mgr.initialize(SandboxConfig::strict()).await.unwrap();

        let wrapped = mgr.wrap_command("cargo build");
        assert!(mgr.guard(&wrapped).is_ok());
    }

    #[tokio::test]
    async fn guard_signals_auto_allow() {
        let mut mgr = SandboxManager::new();
        let config = SandboxConfig {
            auto_allow_bash_if_sandboxed: true,
            allow_unsandboxed_commands: false,
            ..SandboxConfig::default()
        };
        mgr.initialize(config).await.unwrap();

        let wrapped = mgr.wrap_command("cargo build");
        assert!(mgr.guard(&wrapped).unwrap());
    }

    #[tokio::test]
    async fn guard_sandboxed_but_no_auto_allow() {
        let mut mgr = SandboxManager::new();
        mgr.initialize(SandboxConfig::strict()).await.unwrap();

        let wrapped = mgr.wrap_command("cargo build");
        assert!(!mgr.guard(&wrapped).unwrap());
    }

    #[tokio::test]
    async fn guard_allows_excluded_command() {
        let mut mgr = SandboxManager::new();
        let config = SandboxConfig {
            excluded_commands: vec!["git push".into(), "gh pr".into()],
            allow_unsandboxed_commands: false,
            ..SandboxConfig::default()
        };
        mgr.initialize(config).await.unwrap();

        assert!(mgr.guard("git push origin main").is_ok());
        assert!(mgr.guard("gh pr create").is_ok());
        assert!(mgr.guard("rm -rf /").is_err());
    }

    #[tokio::test]
    async fn guard_skips_when_disabled() {
        let mut mgr = SandboxManager::new();
        mgr.initialize(SandboxConfig {
            enabled: false,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(mgr.guard("rm -rf /").is_ok());
    }

    #[tokio::test]
    async fn guard_skips_when_unsandboxed_allowed() {
        let mut mgr = SandboxManager::new();
        mgr.initialize(SandboxConfig {
            allow_unsandboxed_commands: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(mgr.guard("rm -rf /").is_ok());
    }
}
