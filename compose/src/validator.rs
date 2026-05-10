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

#[cfg(test)]
mod tests {
    use super::*;
    use types::{AgentRef, WorkspaceMeta};

    fn make_compose(ids: Vec<&str>, workdir: &str) -> ComposeFile {
        ComposeFile {
            workspace: WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: workdir.into(),
            },
            agents: ids.into_iter().map(|id| AgentRef { id: id.into() }).collect(),
        }
    }

    #[test]
    fn valid_compose_passes() {
        let compose = make_compose(vec!["a", "b", "c"], "./project");
        assert!(validate(&compose).is_ok());
    }

    #[test]
    fn duplicate_ids_fail() {
        let compose = make_compose(vec!["a", "b", "a"], "./project");
        assert!(validate(&compose).is_err());
    }

    #[test]
    fn dotdot_workdir_fails() {
        let compose = make_compose(vec!["a"], "../escape");
        assert!(validate(&compose).is_err());
    }
}
