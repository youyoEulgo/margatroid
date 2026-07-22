//! 公共 Roster 生成器——根据成员库生成团队目录

use types::MemberDef;

/// 生成完整 roster 文本
pub fn generate(members: &[&MemberDef]) -> String {
    let mut buf = String::from("Team members:\n");
    for m in members {
        buf.push_str(&format!(
            "- {}: {} (identity: {:?})\n  Skills: {}\n",
            m.id,
            first_sentence(&m.soul),
            m.identity,
            summarize_skills(&m.skills),
        ));
    }
    buf
}

/// 为指定成员生成个性化团队上下文（排除自己）
pub fn generate_for_agent(members: &[&MemberDef], agent_id: &str) -> String {
    let mut buf = String::new();
    buf.push_str("You are part of a team. Here are your colleagues:\n\n");

    let others: Vec<_> = members.iter().filter(|a| a.id != agent_id).collect();
    if others.is_empty() {
        buf.push_str("(You are the only member of this team.)\n");
        return buf;
    }

    for m in others {
        buf.push_str(&format!(
            "**{}** ({:?}) — {}\n   Skills: {}\n\n",
            m.id,
            m.identity,
            first_sentence(&m.soul),
            summarize_skills(&m.skills),
        ));
    }

    buf.push_str("If you need help that falls within a colleague's skills, you may delegate tasks to them via the delegation board.\n");
    buf
}

fn first_sentence(prompt: &str) -> String {
    for (i, ch) in prompt.char_indices() {
        if ch == '.' || ch == '\u{3002}' || ch == '\n' {
            let end = i + ch.len_utf8();
            let s = prompt[..end].trim().to_string();
            if !s.is_empty() && s != "." {
                return s;
            }
        }
    }
    let max_len = 80;
    if prompt.len() <= max_len {
        prompt.trim().to_string()
    } else {
        format!("{}...", &prompt[..max_len].trim())
    }
}

fn summarize_skills(skills: &[String]) -> String {
    if skills.is_empty() {
        "none".to_string()
    } else {
        skills.join(", ")
    }
}
