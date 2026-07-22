//! Compose 文件解析与校验
//!
//! Margatroid 的 compose 文件定义了一个 Workspace 中
//! 所有 AI 智能体实例及其协作关系。本 crate 提供：
//!
//! - `parser` — TOML 解析与字段级校验
//! - `validator` — 跨字段引用完整性校验
//! - `roster` — 公共团队成员目录生成

pub mod parser;
pub mod roster;
pub mod validator;

use anyhow::Result;
use std::path::Path;
use types::ComposeFile;

/// 完整的 compose 文件加载流程：
/// 解析 → 校验 → 返回 ComposeFile
pub fn load(path: impl AsRef<Path>) -> Result<ComposeFile> {
    let compose = parser::parse_file(path.as_ref())?;
    validator::validate(&compose)?;
    Ok(compose)
}

/// 从字符串加载 compose（用于测试和嵌入式场景）
pub fn load_str(content: &str) -> Result<ComposeFile> {
    let compose = parser::parse_str(content)?;
    validator::validate(&compose)?;
    Ok(compose)
}
