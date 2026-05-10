//! 成员库类型定义

use serde::{Deserialize, Serialize};

/// 成员身份
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Identity {
    User,
    Manager,
    Member,
}

/// 成员库中的成员定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberDef {
    pub id: String,
    pub identity: Identity,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub skills: Vec<String>,
    /// 从 SOUL.md 加载的人格/系统提示词
    #[serde(skip)]
    pub soul: String,
}
