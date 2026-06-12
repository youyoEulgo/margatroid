//! Context — 提示词组装
//!
//! 从 board 迁移过来的提示词拼装逻辑。
//! assemble_prompt 构造 6 部分上下文：
//! 1. 系统提示词
//! 2. 团队成员名录
//! 3. 团队工作日志
//! 4. 委托链上下文
//! 5. 人格提示词（soul）
//! 6. 个人记忆
//! 7. 当前任务

use types::message::{ChatMessage, MessageContent, Role};
use types::MemberProfile;

use crate::board::{ChainEntry, TaskChain};
use crate::memory::SqliteMemory;

/// 组装 LLM 上下文消息
pub fn assemble_prompt(
    system_prompt: &str,
    member_profiles: &[MemberProfile],
    worklog: &str,
    chain: &TaskChain,
    soul: &str,
    memories: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    // 1. 系统提示词（最前面）
    if !system_prompt.is_empty() {
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(system_prompt.to_string()),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });
    }

    // 2. 团队成员名录
    if !member_profiles.is_empty() {
        let mut roster = String::new();
        for profile in member_profiles {
            roster.push_str(&format!("- {} ({})", profile.id, profile.display_name));
            if !profile.tags.is_empty() {
                roster.push_str(&format!(" — 技能: {}", profile.tags.join(", ")));
            }
            roster.push('\n');
        }
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!("--- 团队成员 ---\n{}", roster)),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });
    }

    // 3. 团队工作日志
    if !worklog.is_empty() {
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!("--- 团队工作日志 ---\n{}", worklog)),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });
    }

    // 4. 委托链上下文
    let chain_text = format_chain(chain);
    messages.push(ChatMessage {
        role: Role::User,
        content: MessageContent::Text(format!("--- 委托链上下文 ---\n{}", chain_text)),
        name: None,
        tool_calls: None,
        reasoning_content: None,
    });

    // 5. 人格提示词（system 消息）
    messages.push(ChatMessage {
        role: Role::System,
        content: MessageContent::Text(soul.to_string()),
        name: None,
        tool_calls: None,
        reasoning_content: None,
    });

    // 6. 个人记忆
    if !memories.is_empty() {
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!("--- 你的相关记忆 ---\n{}", memories)),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });
    }

    // 7. 当前任务（永远最后）
    if let Some(task) = chain.current_task() {
        let mut task_desc = format!("当前任务: {}", task.brief);
        if !task.detail.is_empty() {
            task_desc.push_str(&format!("\n详情: {}", task.detail));
        }
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(task_desc),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });
    }

    messages
}

/// 格式化任务链为文本
pub fn format_chain(chain: &TaskChain) -> String {
    let mut lines = Vec::new();
    for (i, entry) in chain.entries.iter().enumerate() {
        let marker = if i == chain.head { "→ " } else { "  " };
        match entry {
            ChainEntry::Delegate { task, .. } => {
                let short_id = if task.id.is_empty() {
                    "(root)"
                } else {
                    &task.id[..task.id.len().min(8)]
                };
                lines.push(format!(
                    "{}[委托 {}] {} 委托 {}: {}",
                    marker, short_id, task.from, task.to, task.brief
                ));
                if !task.detail.is_empty() {
                    lines.push(format!("  详情: {}", task.detail));
                }
                if let Some(ref pid) = task.parent_id {
                    let short_pid = &pid[..pid.len().min(8)];
                    lines.push(format!("  上级: {}", short_pid));
                }
            }
            ChainEntry::Outcome {
                delegate_idx,
                result,
            } => {
                let status = if result.done {
                    "完成"
                } else {
                    "阶段性产出"
                };
                lines.push(format!(
                    "{}[产出 idx={}] {} — {}",
                    marker, delegate_idx, result.summary, status
                ));
            }
        }
    }
    lines.join("\n")
}

/// 格式化 worklog（最近 20 条）
pub fn format_worklog(db: &SqliteMemory) -> String {
    let entries = db.recent(20);
    if entries.is_empty() {
        return String::new();
    }
    entries
        .iter()
        .map(|e| {
            let short_id = &e.delegation_id[..e.delegation_id.len().min(8)];
            format!("[{}] {} — {}", short_id, e.agent_id, e.summary)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::DelegationTask;

    #[test]
    fn test_format_chain_single_task() {
        let task = DelegationTask {
            id: "abc123".to_string(),
            from: "user".to_string(),
            to: "alice".to_string(),
            brief: "测试任务".to_string(),
            detail: "详细描述".to_string(),
            parent_id: None,
        };
        let chain = TaskChain {
            entries: vec![ChainEntry::Delegate {
                task,
                parent_idx: 0,
            }],
            head: 0,
        };

        let text = format_chain(&chain);
        assert!(text.contains("→ [委托"));
        assert!(text.contains("user 委托 alice: 测试任务"));
        assert!(text.contains("详情: 详细描述"));
    }

    #[test]
    fn test_assemble_prompt_basic() {
        let profiles = vec![MemberProfile {
            id: "alice".to_string(),
            display_name: "Alice".to_string(),
            tags: vec!["编程".to_string()],
        }];

        let task = DelegationTask {
            id: "task1".to_string(),
            from: "user".to_string(),
            to: "alice".to_string(),
            brief: "写代码".to_string(),
            detail: String::new(),
            parent_id: None,
        };
        let chain = TaskChain {
            entries: vec![ChainEntry::Delegate {
                task,
                parent_idx: 0,
            }],
            head: 0,
        };

        let messages = assemble_prompt(
            "系统提示",
            &profiles,
            "",
            &chain,
            "你是 Alice",
            "记忆内容",
        );

        // 验证消息顺序
        assert!(messages.len() >= 5);

        // 第一条消息应该包含系统提示
        match &messages[0].content {
            MessageContent::Text(text) => assert!(text.contains("系统提示")),
            _ => panic!("Expected text content"),
        }

        // 第二条消息应该包含团队成员
        match &messages[1].content {
            MessageContent::Text(text) => assert!(text.contains("团队成员")),
            _ => panic!("Expected text content"),
        }

        // 应该有包含 soul 的消息
        assert!(messages.iter().any(|m| match &m.content {
            MessageContent::Text(text) => text.contains("你是 Alice"),
            _ => false,
        }));
    }
}
