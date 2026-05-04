//! Compose 文件校验器
//!
//! 在基本解析完成后，对跨字段的引用完整性进行检查：
//! - depends_on 中引用的 agent id 是否真实存在
//! - 是否存在重复的 agent id
//! - workdir 路径是否安全（不包含向上穿越）

use anyhow::{Result, bail};
use std::collections::HashSet;
use types::ComposeFile;

/// 对 compose 文件执行全量校验
pub fn validate(compose: &ComposeFile) -> Result<()> {
    validate_unique_ids(compose)?;
    validate_depends_on(compose)?;
    validate_workdir(&compose.workspace.workdir)?;
    Ok(())
}

/// 校验 agent id 唯一性
fn validate_unique_ids(compose: &ComposeFile) -> Result<()> {
    let mut seen = HashSet::new();
    for agent in &compose.agents {
        if !seen.insert(&agent.id) {
            bail!("重复的 agent id: '{}'，每个 agent 的 id 必须唯一", agent.id);
        }
    }
    Ok(())
}

/// 校验 depends_on 中引用的 id 都真实存在
fn validate_depends_on(compose: &ComposeFile) -> Result<()> {
    let valid_ids: HashSet<&str> = compose.agents.iter().map(|a| a.id.as_str()).collect();

    for agent in &compose.agents {
        for dep in &agent.depends_on {
            if !valid_ids.contains(dep.as_str()) {
                bail!(
                    "agent '{}' 的 depends_on 引用了不存在的 agent id: '{}'。可用的 agent id: {:?}",
                    agent.id,
                    dep,
                    valid_ids.iter().collect::<Vec<_>>()
                );
            }
            // 不允许自依赖
            if dep == &agent.id {
                bail!("agent '{}' 不能 depends_on 自己", agent.id);
            }
        }
    }

    Ok(())
}

/// 校验 workdir 路径安全性
///
/// 拒绝包含 `..` 路径组件的 workdir，防止沙箱逃逸。
/// workdir 本身无需在解析时存在（可能在 compose 文件所在目录下创建）。
fn validate_workdir(workdir: &str) -> Result<()> {
    if workdir.is_empty() {
        bail!("workdir 不能为空");
    }

    // 检查是否包含向上穿越的路径组件
    for component in workdir.split(&['/', '\\']) {
        if component == ".." {
            bail!(
                "workdir 不能包含 '..' 路径组件，这可能导致沙箱逃逸: {}",
                workdir
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::{AgentDef, WorkspaceMeta};

    fn make_compose(agents: Vec<AgentDef>) -> ComposeFile {
        ComposeFile {
            workspace: WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: "./project".into(),
            },
            agents,
        }
    }

    fn make_agent(id: &str, deps: Vec<&str>) -> AgentDef {
        AgentDef {
            id: id.into(),
            provider: "OpenRouter".into(),
            model: "gpt".into(),
            system_prompt: "hi".into(),
            skills: vec!["coding".into()],
            depends_on: deps.into_iter().map(String::from).collect(),
            profile: None,
            max_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn valid_compose_passes() {
        let compose = make_compose(vec![
            make_agent("a", vec![]),
            make_agent("b", vec!["a"]),
        ]);
        assert!(validate(&compose).is_ok());
    }

    #[test]
    fn reject_duplicate_ids() {
        let compose = make_compose(vec![
            make_agent("same", vec![]),
            make_agent("same", vec![]),
        ]);
        let err = validate(&compose).unwrap_err();
        assert!(err.to_string().contains("重复"));
    }

    #[test]
    fn reject_missing_depends_on() {
        let compose = make_compose(vec![
            make_agent("a", vec!["b"]),  // b 不存在
            make_agent("c", vec![]),
        ]);
        let err = validate(&compose).unwrap_err();
        assert!(err.to_string().contains("depends_on"));
        assert!(err.to_string().contains("b"));
    }

    #[test]
    fn reject_self_dependency() {
        let compose = make_compose(vec![make_agent("a", vec!["a"])]);
        let err = validate(&compose).unwrap_err();
        assert!(err.to_string().contains("自己"));
    }

    #[test]
    fn reject_workdir_with_dotdot() {
        let compose = ComposeFile {
            workspace: WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: "../escape".into(),
            },
            agents: vec![],
        };
        let err = validate(&compose).unwrap_err();
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn reject_empty_workdir() {
        let compose = ComposeFile {
            workspace: WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: "".into(),
            },
            agents: vec![],
        };
        let err = validate(&compose).unwrap_err();
        assert!(err.to_string().contains("为空"));
    }

    #[test]
    fn valid_workdirs_pass() {
        for wd in &["./project", "src", "./my/project", "/absolute/path"] {
            let compose = ComposeFile {
                workspace: WorkspaceMeta {
                    name: "test".into(),
                    version: "0.1.0".into(),
                    description: "".into(),
                    workdir: wd.to_string(),
                },
                agents: vec![],
            };
            assert!(validate(&compose).is_ok(), "workdir '{}' should be valid", wd);
        }
    }
}
