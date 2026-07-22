//! Compose 文件校验器——ID 唯一性、workdir 安全性

use anyhow::{Result, bail};
use std::collections::HashSet;
use types::ComposeFile;

pub fn validate(compose: &ComposeFile) -> Result<()> {
    validate_unique_ids(compose)?;
    validate_workdir(&compose.workspace.workdir)?;
    Ok(())
}

fn validate_unique_ids(compose: &ComposeFile) -> Result<()> {
    let mut seen = HashSet::new();
    for agent in &compose.agents {
        if !seen.insert(&agent.id) {
            bail!("重复的成员 ID: '{}'", agent.id);
        }
    }
    Ok(())
}

fn validate_workdir(workdir: &str) -> Result<()> {
    if workdir.is_empty() {
        bail!("workdir 不能为空");
    }
    for component in workdir.split(&['/', '\\']) {
        if component == ".." {
            bail!("workdir 不能包含 '..' 路径组件: {}", workdir);
        }
    }
    Ok(())
}
