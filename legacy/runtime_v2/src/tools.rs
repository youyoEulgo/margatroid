//! Tools — 工具定义与执行
//!
//! 处理 LLM 的工具调用：bash, delegate, finish, recall, schedule_*

use sandbox::SandboxManager;
use serde_json::Value;
use types::RequestTool;

use crate::board::{DelegationBoard, TaskResult};

// ── 工具定义 ──────────────────────────────────────────────────

pub fn base_tools() -> Vec<RequestTool> {
    vec![
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "bash".into(),
                description: Some("在沙箱环境中执行 shell 命令".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的 shell 命令" }
                    },
                    "required": ["command"]
                }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "delegate".into(),
                description: Some("将子任务委托给团队中的另一个成员".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "目标成员 ID" },
                        "task_summary": { "type": "string", "description": "委托出去的任务简述" },
                        "task_detail": { "type": "string", "description": "委托出去的任务详细描述" },
                        "work_summary": { "type": "string", "description": "一句话简述发委托前做了什么、产出什么、有什么缺漏" },
                        "work_detail": { "type": "string", "description": "发委托前干的事情的具体的思路、做法" }
                    },
                    "required": ["target", "task_summary", "task_detail", "work_summary", "work_detail"]
                }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "recall".into(),
                description: Some("搜索工作日志和个人记忆".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "keyword": { "type": "string", "description": "搜索关键词" }
                    },
                    "required": ["keyword"]
                }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "finish".into(),
                description: Some("完成当前委托，产出最终结果".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string", "description": "一句话简述完成委托做了什么、产出什么" },
                        "detail": { "type": "string", "description": "完成委托的具体思路、做法" }
                    },
                    "required": ["summary", "detail"]
                }),
            },
        },
    ]
}

pub fn manager_tools() -> Vec<RequestTool> {
    let mut tools = base_tools();
    tools.extend(vec![
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_add".into(),
                description: Some("向计划表添加阶段任务".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "指派给谁" },
                        "description": { "type": "string", "description": "任务描述" },
                        "priority": { "type": "integer", "description": "优先级" }
                    },
                    "required": ["target", "description"]
                }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_list".into(),
                description: Some("列出计划表".into()),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_pop".into(),
                description: Some("为指定成员弹出下一个阶段任务".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "成员 ID" }
                    },
                    "required": ["target"]
                }),
            },
        },
        RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_remove".into(),
                description: Some("从计划表删除任务".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer", "description": "条目 ID" }
                    },
                    "required": ["id"]
                }),
            },
        },
    ]);
    tools
}

// ── 工具执行结果 ──────────────────────────────────────────────
pub struct ToolExecResult {
    pub content: String,
    pub should_break: bool,
    pub is_error: bool,
}

/// 执行工具调用
pub async fn execute_tool(
    tool_name: &str,
    arguments: &str,
    sandbox: &SandboxManager,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolExecResult {
    match tool_name {
        "bash" => execute_bash(arguments, sandbox).await,
        "delegate" => execute_delegate(arguments, board, from, reply).await,
        "finish" => execute_finish(arguments, board, from, reply).await,
        "recall" => execute_recall(arguments, board).await,
        "schedule_add" => execute_schedule_add(arguments, board).await,
        "schedule_list" => execute_schedule_list(board).await,
        "schedule_pop" => execute_schedule_pop(arguments, board).await,
        "schedule_remove" => execute_schedule_remove(arguments, board).await,
        _ => ToolExecResult {
            content: format!("未知工具: {}", tool_name),
            should_break: false,
            is_error: true,
        },
    }
}

// ── Bash ──────────────────────────────────────────────────

async fn execute_bash(arguments: &str, sandbox: &SandboxManager) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if command.is_empty() {
        return ToolExecResult {
            content: "command 参数为空".to_string(),
            should_break: false,
            is_error: true,
        };
    }

    let wrapped = sandbox.wrap_command(command);
    if let Err(e) = sandbox.guard(&wrapped) {
        return ToolExecResult {
            content: format!("命令被守卫拒绝: {}", e),
            should_break: false,
            is_error: true,
        };
    }

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&wrapped)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n---stderr---\n");
                }
                result.push_str(&stderr);
            }
            ToolExecResult {
                content: if result.is_empty() {
                    "(无输出)".to_string()
                } else {
                    result
                },
                should_break: false,
                is_error: !out.status.success(),
            }
        }
        Err(e) => ToolExecResult {
            content: format!("执行失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

// ── Delegate ──────────────────────────────────────────────

async fn execute_delegate(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let brief = args.get("brief").and_then(|v| v.as_str()).unwrap_or("");
    let detail = args.get("detail").and_then(|v| v.as_str()).unwrap_or("");

    if to.is_empty() || brief.is_empty() {
        return ToolExecResult {
            content: "to 和 brief 参数必填".to_string(),
            should_break: false,
            is_error: true,
        };
    }

    // 获取当前任务 ID 作为 parent_id
    let parent_id = board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone());

    match board
        .offer(from, to, brief, detail, parent_id.as_deref())
        .await
    {
        Ok(task_id) => {
            let short_id = &task_id[..task_id.len().min(8)];
            tracing::info!("delegate | {} → {} | id={}", from, to, short_id);

            // 记录阶段性产出
            let current_id = parent_id.unwrap_or_default();
            let _ = board
                .result(
                    from,
                    TaskResult {
                        delegation_id: current_id,
                        detail: detail.to_string(),
                        summary: format!("委托给 {}: {}", to, brief),
                        done: false,
                        reply: reply.to_string(),
                    },
                )
                .await;

            ToolExecResult {
                content: format!("已委托给 {}，任务 ID: {}", to, short_id),
                should_break: true,
                is_error: false,
            }
        }
        Err(e) => ToolExecResult {
            content: format!("委托失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

// ── Finish ────────────────────────────────────────────────

async fn execute_finish(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolExecResult {
    let args: Value = if arguments.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(arguments).unwrap_or(Value::Null)
    };

    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("(无摘要)");
    let detail = args.get("detail").and_then(|v| v.as_str()).unwrap_or("");

    let snap = board.chain_snapshot().await;
    let current = snap.current_task();
    let task_from = current.map(|t| t.from.clone()).unwrap_or_default();
    let delegation_id = current.map(|t| t.id.clone()).unwrap_or_default();

    let did = delegation_id.clone();
    match board
        .result(
            from,
            TaskResult {
                delegation_id,
                detail: detail.to_string(),
                summary: summary.to_string(),
                done: true,
                reply: reply.to_string(),
            },
        )
        .await
    {
        Ok(()) => {
            let short_did = &did[..did.len().min(8)];
            tracing::info!("finish | {} ← {} | did={}", task_from, from, short_did);
            ToolExecResult {
                content: format!("完成。摘要: {}", summary),
                should_break: true,
                is_error: false,
            }
        }
        Err(e) => ToolExecResult {
            content: format!("记录完成失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

// ── Recall ────────────────────────────────────────────────

async fn execute_recall(arguments: &str, board: &DelegationBoard) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let keyword = args
        .get("keyword")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if keyword.is_empty() {
        return ToolExecResult {
            content: "keyword 参数为空".to_string(),
            should_break: false,
            is_error: true,
        };
    }

    // 搜索 worklog 和 personal memory
    let worklog_hits = board.db().search(keyword);
    let memory_hits = board.db().recall(keyword);

    let mut lines = Vec::new();
    if !worklog_hits.is_empty() {
        lines.push("工作日志匹配：".to_string());
        for e in &worklog_hits {
            lines.push(format!(
                "  [{}] {}: {}",
                &e.delegation_id[..e.delegation_id.len().min(8)],
                e.agent_id,
                e.summary
            ));
        }
    }
    if !memory_hits.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("个人记忆匹配：".to_string());
        for m in &memory_hits {
            lines.push(format!(
                "  [{}] {} — tags: {}",
                &m.delegation_id[..m.delegation_id.len().min(8)],
                m.summary.chars().take(200).collect::<String>(),
                m.tags.join(", ")
            ));
        }
    }

    let content = if lines.is_empty() {
        format!("未找到与 '{}' 相关的记忆", keyword)
    } else {
        lines.join("\n")
    };

    ToolExecResult {
        content,
        should_break: false,
        is_error: false,
    }
}

// ── Schedule ──────────────────────────────────────────────

async fn execute_schedule_add(arguments: &str, board: &DelegationBoard) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let priority = args.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    if target.is_empty() || desc.is_empty() {
        return ToolExecResult {
            content: "target 和 description 参数必填".to_string(),
            should_break: false,
            is_error: true,
        };
    }

    match board.schedule_add(target, desc, priority) {
        Ok(id) => ToolExecResult {
            content: format!("已添加到计划表，id: {}", id),
            should_break: false,
            is_error: false,
        },
        Err(e) => ToolExecResult {
            content: format!("添加失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

async fn execute_schedule_list(board: &DelegationBoard) -> ToolExecResult {
    let entries = board.schedule_list();
    let content = if entries.is_empty() {
        "计划表为空".to_string()
    } else {
        entries
            .iter()
            .map(|e| format!("[{}] {} → {} (优先级: {})", e.id, e.target, e.description, e.priority))
            .collect::<Vec<_>>()
            .join("\n")
    };
    ToolExecResult { content, should_break: false, is_error: false }
}

async fn execute_schedule_pop(arguments: &str, board: &DelegationBoard) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    if target.is_empty() {
        return ToolExecResult {
            content: "target 参数必填".to_string(),
            should_break: false,
            is_error: true,
        };
    }

    match board.schedule_pop(target) {
        Some(entry) => ToolExecResult {
            content: format!("取出任务 [{}] {} → {}", entry.id, entry.target, entry.description),
            should_break: false,
            is_error: false,
        },
        None => ToolExecResult {
            content: format!("'{}' 没有待处理的计划任务", target),
            should_break: false,
            is_error: false,
        },
    }
}

async fn execute_schedule_remove(arguments: &str, board: &DelegationBoard) -> ToolExecResult {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => {
            return ToolExecResult {
                content: format!("参数解析失败: {}", e),
                should_break: false,
                is_error: true,
            }
        }
    };

    let id = match args.get("id").and_then(|v| v.as_i64()) {
        Some(i) => i,
        None => {
            return ToolExecResult {
                content: "缺少 'id' 参数".to_string(),
                should_break: false,
                is_error: true,
            }
        }
    };

    match board.schedule_remove(id) {
        Ok(()) => ToolExecResult {
            content: format!("已从计划表删除条目 {}", id),
            should_break: false,
            is_error: false,
        },
        Err(e) => ToolExecResult {
            content: format!("删除失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SqliteMemory;
    use std::sync::Arc;

    fn test_board() -> DelegationBoard {
        let temp = std::env::temp_dir().join(format!("test_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(SqliteMemory::open(&temp.to_string_lossy()).unwrap());
        DelegationBoard::new(db, vec!["alice".to_string(), "bob".to_string()])
    }

    #[tokio::test]
    async fn test_execute_finish() {
        let board = test_board();
        // 先创建一个任务
        board
            .offer("user", "alice", "测试", "详情", None)
            .await
            .unwrap();

        let result = execute_finish(
            r#"{"summary":"完成了","detail":"详细结果"}"#,
            &board,
            "alice",
            "回复内容",
        )
        .await;

        assert!(result.should_break);
        assert!(!result.is_error);
        assert!(result.content.contains("完成"));
    }

    #[tokio::test]
    async fn test_execute_delegate() {
        let board = test_board();
        // 创建初始任务
        board
            .offer("user", "alice", "初始任务", "", None)
            .await
            .unwrap();

        let result = execute_delegate(
            r#"{"to":"bob","brief":"子任务","detail":"详情"}"#,
            &board,
            "alice",
            "",
        )
        .await;

        assert!(result.should_break);
        assert!(!result.is_error);
        assert!(result.content.contains("bob"));
    }
}
