//! 成员库加载器
//!
//! 从 ~/.margatroid/members/{id}/ 加载成员定义：
//!   member.toml — 结构化元数据
//!   SOUL.md    — 人格/系统提示词

use anyhow::{Context, Result};
use std::collections::HashMap;
use types::MemberDef;

pub struct MemberLibrary {
    members: HashMap<String, MemberDef>,
}

impl MemberLibrary {
    /// 加载所有成员
    pub fn load() -> Result<Self> {
        let base = paths::margatroid_root()
            .unwrap_or_else(|| std::path::PathBuf::from(".margatroid"))
            .join("members");
        if !base.is_dir() {
            return Ok(Self { members: HashMap::new() });
        }

        let mut members = HashMap::new();
        for entry in std::fs::read_dir(&base)
            .with_context(|| format!("读取成员库失败: {}", base.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let dir = entry.path();
                let toml_path = dir.join("member.toml");
                let soul_path = dir.join("SOUL.md");

                if !toml_path.is_file() {
                    continue;
                }

                let content = std::fs::read_to_string(&toml_path)
                    .with_context(|| format!("读取 {}", toml_path.display()))?;
                let mut def: MemberDef = toml::from_str(&content)
                    .with_context(|| format!("解析 {}", toml_path.display()))?;

                def.soul = if soul_path.is_file() {
                    std::fs::read_to_string(&soul_path)
                        .with_context(|| format!("读取 {}", soul_path.display()))?
                } else {
                    String::new()
                };

                // 确保 skills/ 目录存在
                let _ = std::fs::create_dir_all(dir.join("skills"));

                members.insert(def.id.clone(), def);
            }
        }
        Ok(Self { members })
    }

    /// 按 ID 获取成员定义
    pub fn get(&self, id: &str) -> Option<&MemberDef> {
        self.members.get(id)
    }

    /// 按身份列出成员
    pub fn by_identity(&self, identity: &types::Identity) -> Vec<&MemberDef> {
        self.members
            .values()
            .filter(|m| &m.identity == identity)
            .collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &MemberDef> {
        self.members.values()
    }
}
