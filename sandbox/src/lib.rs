//! Margatroid 沙箱运行时
//!
//! OS 原生的进程级沙箱隔离，不依赖 Docker 或虚拟机。
//! Linux 使用 bubblewrap，macOS 使用 sandbox-exec。
//!
//! # 架构
//!
//! ```ignore
//! let mut sandbox = SandboxManager::new();
//! sandbox.initialize(config).await?;
//! let cmd = sandbox.wrap_command("cargo build");
//! // spawn(cmd) ...
//! sandbox.reset().await?;
//! ```

pub mod config;
mod mandatory;
mod linux;
mod macos;
mod proxy;

use anyhow::Result;
use config::SandboxConfig;
use std::future::Future;
use std::pin::Pin;

/// 沙箱 trait
///
/// 封装平台特定的沙箱实现。上层代码只依赖这个 trait。
pub trait Sandbox: Send + Sync {
    /// 初始化沙箱（启动代理服务器等）
    fn initialize(
        &mut self,
        config: SandboxConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// 将任意 shell 命令包装为沙箱化命令
    ///
    /// 返回的字符串可直接传给 `std::process::Command::new("sh")` 的 `-c` 参数。
    fn wrap_command(&self, cmd: &str) -> String;

    /// 重置沙箱（停止代理、清理临时文件）
    fn reset(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// 平台标识（"linux" / "macos"）
    fn platform(&self) -> &'static str;
}

/// 沙箱管理器
///
/// 根据当前平台自动选择 LinuxSandbox 或 MacOSSandbox。
pub struct SandboxManager {
    inner: Box<dyn Sandbox>,
}

impl SandboxManager {
    /// 创建适合当前平台的沙箱实例
    pub fn new() -> Self {
        Self {
            inner: Self::create_platform(),
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

    pub async fn initialize(&mut self, config: SandboxConfig) -> Result<()> {
        self.inner.initialize(config).await
    }

    pub fn wrap_command(&self, cmd: &str) -> String {
        self.inner.wrap_command(cmd)
    }

    pub async fn reset(&mut self) -> Result<()> {
        self.inner.reset().await
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
