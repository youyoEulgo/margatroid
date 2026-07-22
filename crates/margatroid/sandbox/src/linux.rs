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
        let has_network = !config.network.allowed_domains.is_empty();

        let mut args: Vec<String> = Vec::new();

        // 隔离：网络可控时保留网络访问（流量经代理过滤），否则全隔离
        if has_network {
            args.push("--unshare-pid".into());
            args.push("--unshare-uts".into());
            args.push("--unshare-ipc".into());
        } else {
            args.push("--unshare-all".into());
        }
        args.push("--die-with-parent".into());

        // 代理环境变量（有网络时注入）
        if has_network {
            let http_port = config.network.http_proxy_port.unwrap_or(8888);
            args.push("--setenv".into());
            args.push("HTTP_PROXY".into());
            args.push(format!("http://127.0.0.1:{}", http_port));
            args.push("--setenv".into());
            args.push("HTTPS_PROXY".into());
            args.push(format!("http://127.0.0.1:{}", http_port));
            args.push("--setenv".into());
            args.push("http_proxy".into());
            args.push(format!("http://127.0.0.1:{}", http_port));
            args.push("--setenv".into());
            args.push("https_proxy".into());
            args.push(format!("http://127.0.0.1:{}", http_port));
            args.push("--setenv".into());
            args.push("NO_PROXY".into());
            args.push("localhost,127.0.0.1".into());
        }

        // 只读挂载宿主文件系统
        for dir in &["/usr", "/lib", "/lib64", "/bin", "/etc"] {
            args.push("--ro-bind".into());
            args.push((*dir).into());
            args.push((*dir).into());
        }

        // 可写目录
        if config.filesystem.allow_write.is_empty() {
            // 默认可写 /tmp
            args.push("--bind".into());
            args.push("/tmp".into());
            args.push("/tmp".into());
        } else {
            for path in &config.filesystem.allow_write {
                args.push("--bind".into());
                args.push(path.clone());
                args.push(path.clone());
            }
        }

        // 强制禁止写入的路径
        for path in mandatory::mandatory_deny_write() {
            let expanded = mandatory::expand_home(path);
            args.push("--ro-bind".into());
            args.push("/dev/null".into());
            args.push(expanded);
        }

        args.push("--proc".into());
        args.push("/proc".into());

        // 执行命令
        let escaped_cmd = shell_escape(cmd);
        args.push("--".into());
        args.push("sh".into());
        args.push("-c".into());
        args.push(escaped_cmd);

        build_bwrap_command(&args)
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
            ..Default::default()
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
