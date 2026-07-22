//! 沙箱配置类型

use serde::{Deserialize, Serialize};

/// 沙箱运行时配置
///
/// 用户级默认（`~/.margatroid/sandbox.toml`）与 workspace 级覆盖合并。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// 是否启用沙箱
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// 沙箱内是否自动批准 bash 命令（跳过权限弹窗）
    #[serde(default)]
    pub auto_allow_bash_if_sandboxed: bool,

    /// 是否允许未经沙箱包装的裸命令执行
    /// 默认 false —— 所有命令必须经过 wrap_command()
    #[serde(default)]
    pub allow_unsandboxed_commands: bool,

    /// 免沙箱的命令列表（如 "git push"、"gh pr create"）
    #[serde(default)]
    pub excluded_commands: Vec<String>,

    /// 文件系统规则
    #[serde(default)]
    pub filesystem: FilesystemConfig,

    /// 网络规则
    #[serde(default)]
    pub network: NetworkConfig,
}

fn default_true() -> bool {
    true
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_allow_bash_if_sandboxed: false,
            allow_unsandboxed_commands: false,
            excluded_commands: Vec::new(),
            filesystem: FilesystemConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl SandboxConfig {
    /// 创建严格模式（最安全的默认值）
    pub fn strict() -> Self {
        Self {
            allow_unsandboxed_commands: false,
            auto_allow_bash_if_sandboxed: false,
            ..Default::default()
        }
    }

    /// 用户配置覆盖 workspace 配置
    pub fn merge(mut self, user: &SandboxConfig) -> Self {
        self.enabled = user.enabled;
        if user.auto_allow_bash_if_sandboxed {
            self.auto_allow_bash_if_sandboxed = true;
        }
        if user.allow_unsandboxed_commands {
            self.allow_unsandboxed_commands = true;
        }
        self.excluded_commands
            .extend(user.excluded_commands.clone());
        self.filesystem.merge(&user.filesystem);
        self.network.merge(&user.network);
        self
    }
}

/// 文件系统隔离规则
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesystemConfig {
    #[serde(default)]
    pub deny_read: Vec<String>,

    #[serde(default)]
    pub allow_write: Vec<String>,

    #[serde(default)]
    pub deny_write: Vec<String>,
}

impl FilesystemConfig {
    fn merge(&mut self, other: &FilesystemConfig) {
        self.deny_read.extend(other.deny_read.clone());
        self.allow_write.extend(other.allow_write.clone());
        self.deny_write.extend(other.deny_write.clone());
    }
}

/// 网络隔离规则
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    #[serde(default)]
    pub denied_domains: Vec<String>,

    #[serde(default)]
    pub allow_unix_sockets: Vec<String>,

    #[serde(default)]
    pub allow_local_binding: bool,

    /// HTTP 代理端口（None = 自动分配）
    #[serde(default)]
    pub http_proxy_port: Option<u16>,

    /// SOCKS5 代理端口（None = 自动分配）
    #[serde(default)]
    pub socks_proxy_port: Option<u16>,
}

impl NetworkConfig {
    fn merge(&mut self, other: &NetworkConfig) {
        self.allowed_domains.extend(other.allowed_domains.clone());
        self.denied_domains.extend(other.denied_domains.clone());
        self.allow_unix_sockets
            .extend(other.allow_unix_sockets.clone());
        if other.allow_local_binding {
            self.allow_local_binding = true;
        }
        if other.http_proxy_port.is_some() {
            self.http_proxy_port = other.http_proxy_port;
        }
        if other.socks_proxy_port.is_some() {
            self.socks_proxy_port = other.socks_proxy_port;
        }
    }
}
