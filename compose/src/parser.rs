//! Compose 文件解析器
//!
//! 将 TOML 格式的 compose 文件解析为 ComposeFile 结构体，
//! 并对各个字段做基础合法性校验。

use anyhow::{Context, Result, bail};
use std::path::Path;
use types::{AgentDef, ComposeFile, WorkspaceMeta};

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
        validate_agent(agent)?;
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

fn validate_agent(agent: &AgentDef) -> Result<()> {
    if agent.id.is_empty() {
        bail!("agent id 不能为空");
    }

    validate_segment(&agent.id)?;

    if agent.provider.is_empty() {
        bail!("agent '{}' 的 provider 不能为空", agent.id);
    }

    if agent.model.is_empty() {
        bail!("agent '{}' 的 model 不能为空", agent.id);
    }

    if agent.system_prompt.is_empty() {
        bail!("agent '{}' 的 system_prompt 不能为空", agent.id);
    }

    if agent.skills.is_empty() {
        bail!("agent '{}' 的 skills 列表不能为空", agent.id);
    }

    // 校验 skills 名称合法性
    for skill in &agent.skills {
        if skill.is_empty() {
            bail!("agent '{}' 的 skill 名称不能为空字符串", agent.id);
        }
        if skill.contains(char::is_whitespace) {
            bail!(
                "agent '{}' 的 skill '{}' 不能包含空白字符",
                agent.id,
                skill
            );
        }
    }

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
provider = "OpenRouter"
model = "google/gemini-2.5-flash"
system_prompt = "You are a coder."
skills = ["coding", "testing"]
"#;
        let file = parse_str(toml).unwrap();
        assert_eq!(file.workspace.name, "test-project");
        assert_eq!(file.workspace.version, "0.1.0");
        assert_eq!(file.workspace.workdir, "./project");
        assert_eq!(file.agents.len(), 1);
        assert_eq!(file.agents[0].id, "coder");
        assert_eq!(file.agents[0].skills.len(), 2);
        assert!(file.agents[0].depends_on.is_empty());
    }

    #[test]
    fn parse_with_optional_fields() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./src"

[[agents]]
id = "architect"
provider = "OpenRouter"
model = "claude-sonnet-4"
system_prompt = "You are an architect."
skills = ["design"]
depends_on = []
profile = "architect_profile.md"
max_tokens = 4096
temperature = 0.7
"#;
        let file = parse_str(toml).unwrap();
        let agent = &file.agents[0];
        assert_eq!(agent.profile.as_deref(), Some("architect_profile.md"));
        assert_eq!(agent.max_tokens, Some(4096));
        assert_eq!(agent.temperature, Some(0.7));
    }

    #[test]
    fn reject_empty_name() {
        let toml = r#"
[workspace]
name = ""
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "gpt"
system_prompt = "hi"
skills = ["coding"]
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("name 不能为空"));
    }

    #[test]
    fn reject_whitespace_in_name() {
        let toml = r#"
[workspace]
name = "my project"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "gpt"
system_prompt = "hi"
skills = ["coding"]
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("空白字符"));
    }

    #[test]
    fn reject_missing_skills() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "gpt"
system_prompt = "hi"
skills = []
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("skills"));
    }

    #[test]
    fn reject_missing_provider() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "coder"
provider = ""
model = "gpt"
system_prompt = "hi"
skills = ["coding"]
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("provider"));
    }

    #[test]
    fn reject_invalid_skill_name() {
        let toml = r#"
[workspace]
name = "test"
version = "0.1.0"
workdir = "./project"

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "gpt"
system_prompt = "hi"
skills = ["code review"]
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("空白字符"));
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
provider = "OpenRouter"
model = "gpt"
system_prompt = "hi"
skills = ["coding"]
"#;
        let err = parse_str(toml).unwrap_err();
        assert!(err.to_string().contains("路径分隔符"));
    }
}
