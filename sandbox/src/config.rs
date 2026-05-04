//! 沙箱配置类型

use serde::{Deserialize, Serialize};

/// 沙箱运行时配置
///
/// 由 compose 文件中的 workspace 级别配置生成，
/// 或通过 margatroid.toml 中的 sandbox 段覆盖。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// 文件系统规则
    #[serde(default)]
    pub filesystem: FilesystemConfig,

    /// 网络规则
    #[serde(default)]
    pub network: NetworkConfig,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            filesystem: FilesystemConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

/// 文件系统隔离规则
///
/// 写入默认全禁（allow-only），读取默认全开（deny-then-allow）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    /// 禁止读取的路径（即使默认允许读也会被拒绝）
    #[serde(default)]
    pub deny_read: Vec<String>,

    /// 允许写入的路径（默认全部禁止写入）
    #[serde(default)]
    pub allow_write: Vec<String>,

    /// 无论如何都禁止写入的路径（优先于 allow_write）
    #[serde(default)]
    pub deny_write: Vec<String>,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            deny_read: vec![],
            allow_write: vec![],
            deny_write: vec![],
        }
    }
}

/// 网络隔离规则
///
/// 网络默认全禁（allow-only）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// 允许访问的域名列表（支持 *.example.com 通配符）
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// 明确禁止的域名（优先于 allowed_domains）
    #[serde(default)]
    pub denied_domains: Vec<String>,

    /// 允许的 Unix Domain Socket 路径（用于 Docker socket 等）
    #[serde(default)]
    pub allow_unix_sockets: Vec<String>,

    /// 是否允许绑定本地端口
    #[serde(default)]
    pub allow_local_binding: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            allowed_domains: vec![],
            denied_domains: vec![],
            allow_unix_sockets: vec![],
            allow_local_binding: false,
        }
    }
}
