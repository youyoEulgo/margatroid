//! Bridge 协议类型

use serde::{Deserialize, Serialize};

// ── 常量 ──────────────────────────────────────────────────

pub const DEFAULT_SESSION_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

pub const BRIDGE_LOGIN_INSTRUCTION: &str = "Remote Control is only available with claude.ai subscriptions. \
     Please use `/login` to sign in with your claude.ai account.";

pub const BRIDGE_LOGIN_ERROR: &str = "Error: You must be logged in to use Remote Control.\n\n\
     Remote Control is only available with claude.ai subscriptions. \
     Please use `/login` to sign in with your claude.ai account.";

pub const REMOTE_CONTROL_DISCONNECTED_MSG: &str = "Remote Control disconnected.";

// ── 协议类型 ──────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkDataType {
    Session,
    Healthcheck,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkData {
    #[serde(rename = "type")]
    pub kind: WorkDataType,
    pub id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub environment_id: String,
    pub state: String,
    pub data: WorkData,
    /// base64url 编码的 JSON
    pub secret: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkSecretSource {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_info: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkSecretAuth {
    #[serde(rename = "type")]
    pub kind: String,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkSecret {
    pub version: u32,
    pub session_ingress_token: String,
    pub api_base_url: String,
    #[serde(default)]
    pub sources: Vec<WorkSecretSource>,
    #[serde(default)]
    pub auth: Vec<WorkSecretAuth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_code_args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_variables: Option<std::collections::HashMap<String, String>>,
    /// 服务端驱动的 CCR v2 选择器
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_code_sessions: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDoneStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionActivityType {
    ToolStart,
    Text,
    Result,
    Error,
}

#[derive(Debug, Clone)]
pub struct SessionActivity {
    pub kind: SessionActivityType,
    pub summary: String,
    pub timestamp: u64,
}

/// `claude remote-control` session 目录策略
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpawnMode {
    SingleSession,
    Worktree,
    SameDir,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub dir: String,
    pub machine_name: String,
    pub branch: String,
    pub git_repo_url: Option<String>,
    pub max_sessions: u32,
    pub spawn_mode: SpawnMode,
    pub verbose: bool,
    pub sandbox: bool,
    /// 客户端生成的 UUID，标识本次 bridge 实例
    pub bridge_id: String,
    /// 发送到服务端的 worker_type（例如 "claude_code"）
    pub worker_type: String,
    /// 客户端生成的 UUID，用于幂等环境注册
    pub environment_id: String,
    /// 服务端颁发的 environment_id，用于重连时复用
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_environment_id: Option<String>,
    pub api_base_url: String,
    pub session_ingress_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_timeout_ms: Option<u64>,
}

/// 发送给 session 的 control_response 事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponseEventInner {
    pub subtype: String, // "success"
    pub request_id: String,
    pub response: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponseEvent {
    #[serde(rename = "type")]
    pub kind: String, // "control_response"
    pub response: PermissionResponseEventInner,
}

// ── 错误类型 ──────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Fatal bridge error (status={status}): {message}")]
    Fatal {
        message: String,
        status: u16,
        error_type: Option<String>,
    },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid ID '{label}': contains unsafe characters")]
    InvalidId { label: String },

    #[error("Work secret error: {0}")]
    WorkSecret(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Bridge error: {0}")]
    Other(String),
}

impl BridgeError {
    pub fn fatal(message: impl Into<String>, status: u16) -> Self {
        BridgeError::Fatal {
            message: message.into(),
            status,
            error_type: None,
        }
    }

    pub fn fatal_with_type(
        message: impl Into<String>,
        status: u16,
        error_type: impl Into<String>,
    ) -> Self {
        BridgeError::Fatal {
            message: message.into(),
            status,
            error_type: Some(error_type.into()),
        }
    }

    /// 是否是过期类型的错误
    pub fn is_expired(&self) -> bool {
        if let BridgeError::Fatal { error_type, .. } = self {
            if let Some(et) = error_type {
                return et.contains("expired") || et.contains("lifetime");
            }
        }
        false
    }

    /// 是否是可抑制的 403（权限不足但不影响核心功能）
    pub fn is_suppressible_403(&self) -> bool {
        if let BridgeError::Fatal {
            status, message, ..
        } = self
        {
            if *status == 403 {
                return message.contains("external_poll_sessions")
                    || message.contains("environments:manage");
            }
        }
        false
    }

    pub fn status(&self) -> Option<u16> {
        if let BridgeError::Fatal { status, .. } = self {
            Some(*status)
        } else {
            None
        }
    }
}
