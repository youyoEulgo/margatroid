//! macOS 沙箱实现（sandbox-exec + Seatbelt）
//!
//! 使用系统内置的 `/usr/bin/sandbox-exec` 创建内核级 Seatbelt 策略限制。
//! Agent 继承宿主文件系统，只有声明的路径可写入。
//! 网络流量通过 localhost 代理端口过滤。

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

use crate::config::SandboxConfig;
use crate::mandatory;

/// macOS sandbox-exec 沙箱
#[derive(Debug)]
pub struct MacOSSandbox {
    config: Option<SandboxConfig>,
}

impl MacOSSandbox {
    pub fn new() -> Self {
        Self { config: None }
    }

    /// 检查 sandbox-exec 是否可用（macOS 系统内置，始终可用）
    pub fn is_available() -> bool {
        std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }
}

impl crate::Sandbox for MacOSSandbox {
    fn initialize(
        &mut self,
        config: SandboxConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        if !Self::is_available() {
            tracing::warn!("sandbox-exec not found; sandbox is disabled");
        }
        self.config = Some(config);
        Box::pin(async { Ok(()) })
    }

    fn wrap_command(&self, cmd: &str) -> String {
        let config = self.config.as_ref().expect("sandbox not initialized");

        // 动态生成 Seatbelt 策略（SBPL 格式）
        let profile = generate_seatbelt_profile(config);

        // 写入临时文件并执行
        // 格式: sandbox-exec -f <profile> -- sh -c '<cmd>'
        let escaped_cmd = cmd.replace('\'', "'\\''");
        format!(
            "sandbox-exec -p '(version 1) {}' sh -c 'cd . && {}'",
            profile, escaped_cmd
        )
    }

    fn reset(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        self.config = None;
        Box::pin(async { Ok(()) })
    }

    fn platform(&self) -> &'static str {
        "macos"
    }
}

/// 动态生成 Seatbelt SBPL 策略
///
/// 参照 anthropic-experimental/sandbox-runtime 和 astrid-workspace 的设计。
fn generate_seatbelt_profile(config: &SandboxConfig) -> String {
    let mut rules = Vec::new();

    // 默认拒绝所有写入
    rules.push("(deny default)".to_string());

    // 允许读取整个文件系统
    rules.push("(allow file-read*)".to_string());

    // 允许写入 workdir 和 /tmp
    for path in &config.filesystem.allow_write {
        rules.push(format!(
            "(allow file-write* (subpath \"{}\"))",
            escape_sbpl_path(path)
        ));
    }
    // /tmp 总是可写
    rules.push("(allow file-write* (subpath \"/tmp\"))".to_string());
    rules.push("(allow file-write* (subpath \"/private/tmp\"))".to_string());

    // 强制禁止写入的路径
    for path in mandatory::mandatory_deny_write() {
        let expanded = mandatory::expand_home(path);
        rules.push(format!(
            "(deny file-write* (subpath \"{}\"))",
            escape_sbpl_path(&expanded)
        ));
    }

    // 基础进程权限
    rules.push("(allow process*)".to_string());
    rules.push("(allow sysctl*)".to_string());
    rules.push("(allow signal)".to_string());

    // 网络权限：如果配置了允许的域名，放行到代理端口的 TCP 连接
    if !config.network.allowed_domains.is_empty() {
        let http_port = config.network.http_proxy_port.unwrap_or(8888);
        let socks_port = config.network.socks_proxy_port.unwrap_or(1080);
        rules.push(format!(
            "(allow network* (local ip \"localhost:{}\"))",
            http_port
        ));
        rules.push(format!(
            "(allow network* (local ip \"localhost:{}\"))",
            socks_port
        ));
    }

    rules.join("\n")
}

/// 转义 SBPL 路径中的特殊字符
fn escape_sbpl_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sandbox;
    use crate::config::{FilesystemConfig, NetworkConfig};

    #[test]
    fn generate_profile_with_workdir() {
        let config = SandboxConfig {
            filesystem: FilesystemConfig {
                allow_write: vec!["/workspace".into()],
                ..Default::default()
            },
            network: NetworkConfig::default(),
            ..Default::default()
        };

        let profile = generate_seatbelt_profile(&config);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("/workspace"));
        assert!(profile.contains("/tmp"));
    }

    #[test]
    fn mandatory_deny_paths_in_profile() {
        let config = SandboxConfig::default();
        let profile = generate_seatbelt_profile(&config);
        assert!(profile.contains(".ssh"));
    }

    #[test]
    fn wrap_basic_command() {
        let mut sandbox = MacOSSandbox::new();
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
        assert!(cmd.contains("sandbox-exec"));
        assert!(cmd.contains("echo hello"));
    }

    #[test]
    fn platform_is_macos() {
        let sandbox = MacOSSandbox::new();
        assert_eq!(sandbox.platform(), "macos");
    }
}
