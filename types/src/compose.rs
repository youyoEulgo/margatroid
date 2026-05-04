//! Compose 文件类型定义
//!
//! 对应 Margatroid 的 compose.toml 格式，用于声明式定义
//! 一个 Workspace 中的智能体实例及其协作关系。

use serde::{Deserialize, Serialize};

/// Compose 文件顶层结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeFile {
    pub workspace: WorkspaceMeta,
    #[serde(default)]
    pub agents: Vec<AgentDef>,
}

/// Workspace 元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    /// Workspace 唯一标识
    pub name: String,
    /// 配置版本号
    pub version: String,
    /// 项目描述
    #[serde(default)]
    pub description: String,
    /// 项目信任目录，相对于 compose 文件位置
    pub workdir: String,
}

/// 单个智能体实例定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// 成员唯一标识符
    pub id: String,
    /// AI 服务商名称，对应 margatroid.toml 中的 provider
    pub provider: String,
    /// 模型 ID
    pub model: String,
    /// 角色定义和基本行为边界
    pub system_prompt: String,
    /// 拥有的能力标签列表
    #[serde(default)]
    pub skills: Vec<String>,
    /// 声明式依赖，用于自文档化协作拓扑
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// 详细能力描述文件路径（若不指定则自动使用默认模板）
    #[serde(default)]
    pub profile: Option<String>,
    /// 每次请求的最大 token 数
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 模型温度参数
    #[serde(default)]
    pub temperature: Option<f32>,
}
