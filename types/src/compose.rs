//! Compose 文件类型定义
//!
//! 对应 Margatroid 的 compose.toml 格式。成员定义在成员库中，
//! compose 文件只引用成员 ID。

use serde::{Deserialize, Serialize};

/// Compose 文件顶层结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeFile {
    pub workspace: WorkspaceMeta,
    /// 引用成员库中的成员 ID
    #[serde(default)]
    pub agents: Vec<AgentRef>,
}

/// 成员引用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRef {
    /// 对应成员库中的成员 ID
    pub id: String,
}

/// Workspace 元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub workdir: String,
}
