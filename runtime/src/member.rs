//! Member — 单个团队成员 + Agent trait
//!
//! 封装客户端、沙箱。
//! Agent trait 定义统一调用接口，Member 是唯一实现。
//! chat() 驱动 LLM tool-call loop，通过 Board.assemble_prompt() 获取上下文。

use anyhow::Result;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    FinishReason, Identity, RequestMessage, RequestTool, ResponseChoice,
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::ResponseToolCall,
};

use crate::board::DelegationBoard;
use crate::client::Client;

/// 所有成员（LLM 或 Human）都实现这个 trait
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    fn id(&self) -> &str;
    fn identity(&self) -> &Identity;

    /// 处理一个任务：board 组装上下文 → LLM tool-call loop → 结果 + 总结
    async fn process(&self, board: &DelegationBoard, tools: &[RequestTool]) -> Result<ChatOutcome>;
}

/// chat() 返回值
pub struct ChatOutcome {
    pub result: String,
}

/// execute_tool 返回值 —— (tool 结果文本, 是否应该退出循环)
type ToolResult = (String, bool);

// ── Member ───────────────────────────────────────────────────

pub struct Member {
    pub id: String,
    soul: String,
    identity: Identity,
    client: Client,
    sandbox: Arc<RwLock<SandboxManager>>,
}

impl Member {
    pub fn new(
        id: &str,
        soul: String,
        identity: Identity,
        client: Client,
        sandbox: Arc<RwLock<SandboxManager>>,
    ) -> Self {
        Self {
            id: id.to_string(),
            soul,
            identity,
            client,
            sandbox,
        }
    }
}

#[async_trait::async_trait]
impl Agent for Member {
    fn id(&self) -> &str {
        &self.id
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }

    async fn process(&self, board: &DelegationBoard, tools: &[RequestTool]) -> Result<ChatOutcome> {
        self.chat(board, tools).await
    }
}

const LOOP_CONSTRAINT: &str = "你必须完成当前委托或发布新委托才能结束";

impl Member {
    async fn chat(&self, board: &DelegationBoard, tools: &[RequestTool]) -> Result<ChatOutcome> {
        let memories = format_memories(board.db(), &self.id);
        let chat_messages = board.assemble_prompt(&self.soul, &memories).await;
        let mut messages: Vec<RequestMessage> = chat_messages
            .into_iter()
            .map(RequestMessage::Chat)
            .collect();

        // 主 tool-call loop
        let final_content = loop {
            let resp = self.client.chat(messages.clone(), tools).await?;
            let choice = resp
                .choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("provider returned empty choices"))?;

            if choice.finish_reason == Some(FinishReason::Stop)
                && choice
                    .message
                    .tool_calls
                    .as_ref()
                    .map_or(true, |v| v.is_empty())
            {
                let bare_text = choice.message.content.clone().unwrap_or_default();
                if !bare_text.is_empty() {
                    save_conversation(board, &self.id, &bare_text).await;
                    publish_msg(board, &bare_text).await;
                }
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(bare_text),
                    name: None,
                    tool_calls: None,
                }));
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(LOOP_CONSTRAINT.to_string()),
                    name: None,
                    tool_calls: None,
                }));
                continue;
            }

            messages.push(request_message_from_choice(choice));

            let text_content = choice.message.content.clone().unwrap_or_default();
            let has_breaking_tool = choice
                .message
                .tool_calls
                .as_ref()
                .map_or(false, |tc| {
                    tc.iter()
                        .any(|t| matches!(t.function.name.as_str(), "finish" | "delegate"))
                });

            if !text_content.is_empty() {
                save_conversation(board, &self.id, &text_content).await;
                if !has_breaking_tool {
                    publish_msg(board, &text_content).await;
                }
            }

            if let Some(tool_calls) = &choice.message.tool_calls {
                let sandbox_guard = self.sandbox.read().await;
                let mut should_break = false;
                let mut break_content = String::new();
                let reply = choice.message.content.clone().unwrap_or_default();

                for tc in tool_calls {
                    let (result, brk) =
                        execute_tool(tc, &sandbox_guard, board, &self.id, &reply).await;

                    messages.push(RequestMessage::Tool(ToolMessage {
                        role: Role::Tool,
                        content: result.clone(),
                        tool_call_id: tc.id.clone(),
                        name: None,
                    }));

                    if brk {
                        should_break = true;
                        break_content = result;
                    }
                }
                drop(sandbox_guard);

                if should_break {
                    break break_content;
                }
            }
        };

        Ok(ChatOutcome {
            result: final_content,
        })
    }
}
// ── Helpers ──────────────────────────────────────────────────

fn request_message_from_choice(choice: &ResponseChoice) -> RequestMessage {
    RequestMessage::Chat(ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Text(choice.message.content.clone().unwrap_or_default()),
        name: None,
        tool_calls: choice.message.tool_calls.clone(),
    })
}

async fn execute_tool(
    tc: &ResponseToolCall,
    sandbox: &SandboxManager,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolResult {
    match tc.function.name.as_str() {
        "bash" => (execute_bash(&tc.function.arguments, sandbox).await, false),
        "delegate" => {
            let result = execute_delegate(&tc.function.arguments, board, from, reply).await;
            (result, true)
        }
        "schedule_add" => (
            execute_schedule_add(&tc.function.arguments, board).await,
            false,
        ),
        "schedule_list" => (execute_schedule_list(board).await, false),
        "schedule_pop" => (
            execute_schedule_pop(&tc.function.arguments, board).await,
            false,
        ),
        "schedule_remove" => (
            execute_schedule_remove(&tc.function.arguments, board).await,
            false,
        ),
        "recall" => (execute_recall(&tc.function.arguments, board).await, false),
        "finish" => (
            execute_finish(&tc.function.arguments, board, from, reply).await,
            true,
        ),
        _ => (format!("未知工具: {}", tc.function.name), false),
    }
}

async fn execute_finish(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("(无摘要)");
    let detail = args.get("detail").and_then(|v| v.as_str()).unwrap_or("");

    let delegation_id = board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone())
        .unwrap_or_default();

    let did = delegation_id.clone();
    match board
        .result(
            from,
            crate::board::TaskResult {
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
            let content = if reply.is_empty() {
                summary.to_string()
            } else {
                reply.to_string()
            };
            board
                .publish_event(crate::board::ChatEvent {
                    event_type: "done".into(),
                    content,
                    delegation_id: did,
                })
                .await;
            format!("完成。摘要: {}", summary)
        }
        Err(e) => format!("产出写入失败: {}", e),
    }
}

async fn execute_delegate(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return "缺少 'target' 参数".to_string(),
    };
    let task_summary = match args.get("task_summary").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return "缺少 'task_summary' 参数".to_string(),
    };
    let task_detail = match args.get("task_detail").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return "缺少 'task_detail' 参数".to_string(),
    };
    let work_summary = match args.get("work_summary").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return "缺少 'work_summary' 参数".to_string(),
    };
    let work_detail = match args.get("work_detail").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return "缺少 'work_detail' 参数".to_string(),
    };
    let parent_id = board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone());

    // 记录发委托前的产出
    let delegation_id = parent_id.clone().unwrap_or_default();
    let _ = board
        .result(
            from,
            crate::board::TaskResult {
                delegation_id,
                detail: work_detail.to_string(),
                summary: work_summary.to_string(),
                done: false,
                reply: reply.to_string(),
            },
        )
        .await;

    match board
        .offer(
            from,
            target,
            task_summary,
            task_detail,
            parent_id.as_deref(),
        )
        .await
    {
        Ok(task_id) => format!(
            "委托已发布到发布区，task_id: {}\n发委托前总结: {}\n发委托前思路: {}",
            task_id, work_summary, work_detail
        ),
        Err(e) => format!("委托发布失败: {}", e),
    }
}

async fn execute_schedule_add(arguments: &str, board: &DelegationBoard) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let desc = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let priority = args.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    match board.schedule_add(target, desc, priority) {
        Ok(id) => format!("已添加到计划表，id: {}", id),
        Err(e) => format!("添加失败: {}", e),
    }
}

async fn execute_schedule_list(board: &DelegationBoard) -> String {
    let entries = board.schedule_list();
    if entries.is_empty() {
        return "计划表为空".to_string();
    }
    entries
        .iter()
        .map(|e| {
            format!(
                "[{}] {} → {} (优先级: {})",
                e.id, e.target, e.description, e.priority
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn execute_schedule_pop(arguments: &str, board: &DelegationBoard) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    match board.schedule_pop(target) {
        Some(entry) => format!(
            "取出任务 [{}] {} → {}",
            entry.id, entry.target, entry.description
        ),
        None => format!("'{}' 没有待处理的计划任务", target),
    }
}

async fn execute_schedule_remove(arguments: &str, board: &DelegationBoard) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let id = match args.get("id").and_then(|v| v.as_i64()) {
        Some(i) => i,
        None => return "缺少 'id' 参数".to_string(),
    };
    match board.schedule_remove(id) {
        Ok(()) => format!("已从计划表删除条目 {}", id),
        Err(e) => format!("删除失败: {}", e),
    }
}

async fn execute_recall(arguments: &str, board: &DelegationBoard) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let keyword = match args.get("keyword").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return "缺少 'keyword' 参数".to_string(),
    };
    board.recall(keyword)
}

async fn execute_bash(arguments: &str, sandbox: &SandboxManager) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let command = match args.get("command").and_then(|v| v.as_str()) {
        Some(cmd) => cmd.to_string(),
        None => return "缺少 'command' 参数".to_string(),
    };
    let wrapped = sandbox.wrap_command(&command);
    if let Err(e) = sandbox.guard(&wrapped) {
        return format!("命令被守卫拒绝: {}", e);
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
            let exit = out.status.code().unwrap_or(-1);
            if exit == 0 {
                format!("退出码: 0\n{}", stdout)
            } else {
                format!("退出码: {}\nstdout:\n{}\nstderr:\n{}", exit, stdout, stderr)
            }
        }
        Err(e) => format!("执行失败: {}", e),
    }
}

fn format_memories(db: &crate::memory::SqliteMemory, member_id: &str) -> String {
    let my_log = match db.worklog_by_agent(member_id, 10) {
        Ok(log) => log,
        Err(e) => {
            tracing::warn!("无法查询成员 '{}' 的工作日志: {}", member_id, e);
            return "(暂无记录)".to_string();
        }
    };
    let delegation_ids: Vec<String> = my_log.iter().map(|e| e.delegation_id.clone()).collect();
    let memories = db.personal_by_delegations(&delegation_ids);
    if memories.is_empty() {
        return "(暂无记录)".to_string();
    }
    memories
        .iter()
        .map(|m| {
            format!(
                "[{}] {} — tags: {}",
                m.delegation_id,
                m.summary,
                m.tags.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn save_conversation(board: &DelegationBoard, agent_id: &str, content: &str) {
    let delegation_id = board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone())
        .unwrap_or_default();
    if delegation_id.is_empty() {
        return;
    }
    let _ = board
        .db()
        .conversation_add(&delegation_id, agent_id, content);
}

async fn publish_msg(board: &DelegationBoard, content: &str) {
    let delegation_id = board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone())
        .unwrap_or_default();
    if delegation_id.is_empty() {
        return;
    }
    let _ = board
        .publish_event(crate::board::ChatEvent {
            event_type: "message".into(),
            content: content.to_string(),
            delegation_id,
        })
        .await;
}
