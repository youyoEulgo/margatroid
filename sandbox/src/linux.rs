//! Linux 沙箱实现（bubblewrap）
//!
//! 使用 `bwrap` 创建挂载/PID/网络命名空间隔离。
//! Agent 继承宿主文件系统为只读，只有 workdir 和 /tmp 可写。
//! 网络流量通过 Unix Domain Socket 桥接到宿主代理服务器。

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

use crate::config::SandboxConfig;
use crate::mandatory;

/// Linux bubblewrap 沙箱
#[derive(Debug)]
pub struct LinuxSandbox {
    config: Option<SandboxConfig>,
}

impl LinuxSandbox {
    pub fn new() -> Self {
        Self { config: None }
    }

    /// 检查 bwrap 是否可用
    pub fn is_available() -> bool {
        std::process::Command::new("bwrap")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl crate::Sandbox for LinuxSandbox {
    fn initialize(
        &mut self,
        config: SandboxConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        if !Self::is_available() {
            tracing::warn!("bwrap not found — sandbox is disabled");
        }
        self.config = Some(config);
        Box::pin(async { Ok(()) })
    }

    fn wrap_command(&self, cmd: &str) -> String {
        let config = self.config.as_ref().expect("sandbox not initialized");

        let mut args: Vec<String> = Vec::new();

        // 基础隔离
        args.push("--unshare-all".into());
        args.push("--die-with-parent".into());

        // 只读挂载宿主文件系统
        args.push("--ro-bind".into());
        args.push("/usr".into());
        args.push("/usr".into());
        args.push("--ro-bind".into());
        args.push("/lib".into());
        args.push("/lib".into());
        args.push("--ro-bind".into());
        args.push("/lib64".into());
        args.push("/lib64".into());
        args.push("--ro-bind".into());
        args.push("/bin".into());
        args.push("/bin".into());
        args.push("--ro-bind".into());
        args.push("/etc".into());
        args.push("/etc".into());

        // 可写目录（allow_write 配置）
        for path in &config.filesystem.allow_write {
            args.push("--bind".into());
            args.push(path.clone());
            args.push(path.clone());
        }

        // 强制禁止写入的路径
        for path in mandatory::mandatory_deny_write() {
            let expanded = mandatory::expand_home(path);
            args.push("--ro-bind".into());
            args.push("/dev/null".into());
            args.push(expanded);
        }

        // /tmp 可写（如果不在 allow_write 中）
        args.push("--bind".into());
        args.push("/tmp".into());
        args.push("/tmp".into());

        // 创建 /proc
        args.push("--proc".into());
        args.push("/proc".into());

        // 网络隔离：如果没有配置代理，直接用 --unshare-net 阻断
        if config.network.allowed_domains.is_empty() {
            // 无网络权限
        } else {
            // 通过 Unix Domain Socket 桥接到代理（Phase 3 完成）
        }

        // 执行命令
        args.push("--".into());
        args.push("sh".into());
        args.push("-c".into());
        args.push(format!("cd {} && {}", ".", cmd));

        let bwrap_cmd = build_bwrap_command(&args);
        format!("sh -c '{}'", shell_escape(&bwrap_cmd))
    }

    fn reset(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        self.config = None;
        Box::pin(async { Ok(()) })
    }

    fn platform(&self) -> &'static str {
        "linux"
    }
}

/// 构建 bwrap 命令行
fn build_bwrap_command(args: &[String]) -> String {
    let mut cmd = String::from("bwrap");
    for arg in args {
        cmd.push(' ');
        cmd.push_str(arg);
    }
    cmd
}

/// 基本的 shell 转义（防止命令注入）
fn shell_escape(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"");
    format!("'{}'", escaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sandbox;
    use crate::config::{FilesystemConfig, NetworkConfig};

    #[test]
    fn wrap_basic_command() {
        let mut sandbox = LinuxSandbox::new();
        let config = SandboxConfig {
            filesystem: FilesystemConfig {
                allow_write: vec!["/workspace".into()],
                ..Default::default()
            },
            network: NetworkConfig::default(),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(sandbox.initialize(config)).unwrap();

        let cmd = sandbox.wrap_command("echo hello");
        assert!(cmd.contains("bwrap"));
        assert!(cmd.contains("--unshare-all"));
        assert!(cmd.contains("echo hello"));
        assert!(cmd.contains("/workspace"));
    }

    #[test]
    fn mandatory_deny_paths_included() {
        let mut sandbox = LinuxSandbox::new();
        let config = SandboxConfig::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(sandbox.initialize(config)).unwrap();

        let cmd = sandbox.wrap_command("ls");
        // 强制 deny 路径应被映射到 /dev/null
        assert!(cmd.contains(".ssh"));
    }

    #[test]
    fn platform_is_linux() {
        let sandbox = LinuxSandbox::new();
        assert_eq!(sandbox.platform(), "linux");
    }
}
