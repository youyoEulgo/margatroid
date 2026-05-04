//! 公共 Roster Skill 生成器
//!
//! 根据 compose 文件中定义的 agent 列表，自动生成一份
//! "团队成员目录"文本，注入到每个 agent 的可用 skill 列表中。
//!
//! Roster 是成员发现机制的第一层——轻量级，每个成员启动时就
//! 知道团队中有谁、各自大致做什么。详细能力查询走第二层的 Profile。

use types::{AgentDef, ComposeFile};

/// 生成 roster 文本
///
/// 格式示例：
/// ```text
/// Team members:
/// - architect: Software architect, skilled in system design and code review
/// - coder: Programmer, skilled in coding and testing
/// - reviewer: Code reviewer, skilled in code review and security review
/// ```
pub fn generate(compose: &ComposeFile) -> String {
    let mut buf = String::new();

    buf.push_str("Team members:\n");

    for agent in &compose.agents {
        buf.push_str(&format_agent_entry(agent));
    }

    buf
}

/// 生成单个成员的 roster 条目
fn format_agent_entry(agent: &AgentDef) -> String {
    let skills_summary = summarize_skills(&agent.skills);
    format!(
        "- {}: {}\n  Skills: {}\n",
        agent.id,
        first_sentence(&agent.system_prompt),
        skills_summary,
    )
}

/// 提取 system_prompt 的第一句话作为角色简述
fn first_sentence(prompt: &str) -> String {
    // 按中文句号、英文句号、换行符分割，取第一段
    for (i, ch) in prompt.char_indices() {
        if ch == '.' || ch == '\u{3002}' || ch == '\n' {
            let end = i + ch.len_utf8();
            let s = prompt[..end].trim().to_string();
            if !s.is_empty() && s != "." {
                return s;
            }
        }
    }
    // 没有找到终止符，返回整个 prompt（截断到合理长度）
    let max_len = 80;
    if prompt.len() <= max_len {
        prompt.trim().to_string()
    } else {
        format!("{}...", &prompt[..max_len].trim())
    }
}

/// 将 skills 列表格式化为 human-readable 字符串
fn summarize_skills(skills: &[String]) -> String {
    if skills.is_empty() {
        return "none".to_string();
    }
    skills.join(", ")
}

/// 为指定 agent 生成个性化的 team context
///
/// 这个函数生成的是注入到单个 agent prompt 中的团队信息，
/// 列举了除自己之外的所有成员。
pub fn generate_for_agent(compose: &ComposeFile, agent_id: &str) -> String {
    let mut buf = String::new();

    buf.push_str("You are part of a team. Here are your colleagues:\n\n");

    let others: Vec<_> = compose
        .agents
        .iter()
        .filter(|a| a.id != agent_id)
        .collect();

    if others.is_empty() {
        buf.push_str("(You are the only member of this team.)\n");
        return buf;
    }

    for agent in others {
        buf.push_str(&format!(
            "**{}** — {}\n   Skills: {}\n\n",
            agent.id,
            first_sentence(&agent.system_prompt),
            summarize_skills(&agent.skills),
        ));
    }

    buf.push_str("If you need help that falls within a colleague's skills, you may delegate tasks to them via the delegation board.\n");

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_compose() -> ComposeFile {
        ComposeFile {
            workspace: types::WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: "./project".into(),
            },
            agents: vec![
                AgentDef {
                    id: "architect".into(),
                    provider: "OpenRouter".into(),
                    model: "claude-sonnet-4".into(),
                    system_prompt: "You are a software architect. You design systems.".into(),
                    skills: vec!["design".into(), "code-review".into()],
                    depends_on: vec![],
                    profile: None,
                    max_tokens: None,
                    temperature: None,
                },
                AgentDef {
                    id: "coder".into(),
                    provider: "OpenRouter".into(),
                    model: "gemini-flash".into(),
                    system_prompt: "You are a programmer. You write code.".into(),
                    skills: vec!["coding".into(), "testing".into()],
                    depends_on: vec!["architect".into()],
                    profile: None,
                    max_tokens: None,
                    temperature: None,
                },
            ],
        }
    }

    #[test]
    fn test_generate_roster() {
        let compose = sample_compose();
        let roster = generate(&compose);
        assert!(roster.contains("architect"));
        assert!(roster.contains("coder"));
        assert!(roster.contains("design, code-review"));
        assert!(roster.contains("coding, testing"));
    }

    #[test]
    fn test_generate_for_agent_excludes_self() {
        let compose = sample_compose();
        let context = generate_for_agent(&compose, "coder");
        assert!(!context.contains("**coder**"));
        assert!(context.contains("**architect**"));
        assert!(context.contains("delegate"));
    }

    #[test]
    fn test_first_sentence_english() {
        assert_eq!(
            first_sentence("You are a programmer. You write code."),
            "You are a programmer."
        );
    }

    #[test]
    fn test_first_sentence_chinese() {
        assert_eq!(
            first_sentence("你是一个程序员。负责编写代码。"),
            "你是一个程序员。"
        );
    }

    #[test]
    fn test_first_sentence_newline() {
        assert_eq!(
            first_sentence("Role: coder\nYou write code."),
            "Role: coder"
        );
    }

    #[test]
    fn test_first_sentence_no_terminator() {
        let short = "You are a coder";
        assert_eq!(first_sentence(short), short);
    }
}
