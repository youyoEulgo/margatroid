//! Compose 文件解析器

use anyhow::{Context, Result, bail};
use std::path::Path;
use types::{AgentRef, ComposeFile, WorkspaceMeta};

/// 从文件路径解析 compose 文件
pub fn parse_file(path: impl AsRef<Path>) -> Result<ComposeFile> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 compose 文件: {}", path.display()))?;
    parse_str(&content)
}

/// 从字符串解析 compose 文件
pub fn parse_str(content: &str) -> Result<ComposeFile> {
    let file: ComposeFile = toml::from_str(content)
        .context("TOML 解析失败，请检查 compose 文件格式")?;

    validate_workspace(&file.workspace)?;

    for agent in &file.agents {
        validate_agent_ref(agent)?;
    }

    Ok(file)
}

fn validate_workspace(meta: &WorkspaceMeta) -> Result<()> {
    if meta.name.is_empty() {
        bail!("[workspace] name 不能为空");
    }
    if meta.name.contains('/') || meta.name.contains('\\') {
        bail!("[workspace] name 不能包含路径分隔符: {}", meta.name);
    }
    if meta.name.contains(char::is_whitespace) {
        bail!("[workspace] name 不能包含空白字符: {}", meta.name);
    }
    if meta.version.is_empty() {
        bail!("[workspace] version 不能为空");
    }
    if meta.workdir.is_empty() {
        bail!("[workspace] workdir 不能为空");
    }
    Ok(())
}

fn validate_agent_ref(agent: &AgentRef) -> Result<()> {
    if agent.id.is_empty() {
        bail!("agent id 不能为空");
    }
    validate_segment(&agent.id)?;
    Ok(())
}

fn validate_segment(s: &str) -> Result<()> {
    if s.is_empty() {
        bail!("id 不能为空");
    }
    if s == "." || s == ".." {
        bail!("id 不能为 '.' 或 '..': {}", s);
    }
    if s.contains('/') || s.contains('\\') {
        bail!("id 不能包含路径分隔符: {}", s);
    }
    if s.contains(char::is_whitespace) {
        bail!("id 不能包含空白字符: {}", s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_compose() {
        let toml = r#"
[workspace]
name = "test-project"
version = "0.1.0"
description = "A test workspace"
workdir = "./project"

[[agents]]
id = "coder"
"#;
        let file = parse_str(toml).unwrap();
        assert_eq!(file.workspace.name, "test-project");
        assert_eq!(file.agents.len(), 1);
        assert_eq!(file.agents[0].id, "coder");
    }

    #[test]
    fn reject_empty_id() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = ""
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("id 不能为空"));
    }

    #[test]
    fn reject_invalid_agent_id() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "my/agent"
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("路径分隔符"));
    }
}
