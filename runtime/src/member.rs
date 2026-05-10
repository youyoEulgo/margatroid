//! Member — 单个团队成员
//!
//! 封装模型、供应商、沙箱、委托板。
//! 实现 Agent trait，chat() 驱动 LLM tool-call loop。

use anyhow::Result;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    ChatRequest, DynAiProvider, FinishReason, Identity, RequestMessage, RequestTool,
    ResponseChoice,
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::ResponseToolCall,
};

use crate::agent::Agent;
use crate::board::DelegationBoard;

// ── Member ───────────────────────────────────────────────────

/// 单个团队成员
pub struct Member {
    pub id: String,
    identity: Identity,
    model: String,
    provider: Arc<dyn DynAiProvider>,
    sandbox: Arc<RwLock<SandboxManager>>,
    board: Arc<DelegationBoard>,
}

/// chat() 返回值
pub struct ChatOutcome {
    pub result: String,
    pub summary: String,
}

/// execute_tool 返回值 —— (tool 结果文本, 是否应该退出循环)
type ToolResult = (String, bool);

impl Member {
    pub fn new(
        id: &str,
        identity: Identity,
        model: &str,
        provider: Arc<dyn DynAiProvider>,
        sandbox: Arc<RwLock<SandboxManager>>,
        board: Arc<DelegationBoard>,
    ) -> Self {
        Self {
            id: id.to_string(),
            identity,
            model: model.to_string(),
            provider,
            sandbox,
            board,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
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

    async fn process(
        &self,
        prompt: &str,
        task_description: &str,
        tools: &[RequestTool],
    ) -> Result<ChatOutcome> {
        self.chat(prompt, task_description, tools, None).await
    }
}

const LOOP_CONSTRAINT: &str = "你必须返回当前委托或发布新委托才能结束";

impl Member {
    /// 执行一次对话
    ///
    /// `prompt` 完整提示词。`task_description` 当前任务简述。
    /// `current_task_id` 当前委托 ID，子委托时会作为 parent_id。
    /// `tools` 由调用方决定注入哪些工具。
    pub async fn chat(
        &self,
        prompt: &str,
        _task_description: &str,
        tools: &[RequestTool],
        current_task_id: Option<&str>,
    ) -> Result<ChatOutcome> {
        let mut messages: Vec<RequestMessage> = vec![RequestMessage::Chat(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(prompt.to_string()),
            name: None,
            tool_calls: None,
        })];

        // ── 主 tool-call loop ──
        let (final_content, final_summary) = loop {
            let req = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: if tools.is_empty() {
                    None
                } else {
                    Some(tools.to_vec())
                },
                tool_choice: if tools.is_empty() {
                    None
                } else {
                    Some(types::RequestToolChoice::String("auto".into()))
                },
                ..Default::default()
            };

            let resp = self.provider.chat_boxed(req).await?;
            let choice = resp.choices.first().expect("no response choice");

            // 没有 tool calls → 追加约束消息，继续循环
            if choice.finish_reason == Some(FinishReason::Stop)
                && choice
                    .message
                    .tool_calls
                    .as_ref()
                    .map_or(true, |v| v.is_empty())
            {
                let bare_text = choice.message.content.clone().unwrap_or_default();
                // 保存 assistant 消息
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(bare_text),
                    name: None,
                    tool_calls: None,
                }));
                // 追加约束
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(LOOP_CONSTRAINT.to_string()),
                    name: None,
                    tool_calls: None,
                }));
                continue;
            }

            // 保存 assistant 消息（含 tool_calls）
            messages.push(request_message_from_choice(choice));

            // 执行 tool calls
            if let Some(tool_calls) = &choice.message.tool_calls {
                let sandbox_guard = self.sandbox.read().await;
                let mut should_break = false;
                let mut break_content = String::new();

                for tc in tool_calls {
                    let (result, brk) =
                        execute_tool(tc, &sandbox_guard, &self.board, &self.id, current_task_id)
                            .await;

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
                    break (break_content.clone(), break_content);
                }
            }
        };

        // ── 内联总结（同一上下文，KV cache 不中断）──
        let summary_prompt = format!(
            "请用一两句话总结你刚才完成的工作：做了什么、产出了什么、有什么遗留。\n\n工作结果供你参考：\n{}",
            final_content
        );
        messages.push(RequestMessage::Chat(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(summary_prompt),
            name: None,
            tool_calls: None,
        }));

        let summary_req = ChatRequest {
            model: self.model.clone(),
            messages,
            ..Default::default()
        };

        let summary_resp = self.provider.chat_boxed(summary_req).await?;
        let summary = summary_resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or(&final_summary)
            .to_string();

        Ok(ChatOutcome {
            result: final_content,
            summary,
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
    current_task_id: Option<&str>,
) -> ToolResult {
    match tc.function.name.as_str() {
        "bash" => (execute_bash(&tc.function.arguments, sandbox).await, false),
        "delegate" => {
            let result =
                execute_delegate(&tc.function.arguments, board, from, current_task_id).await;
            (result, true)
        }
        "delegate_reject" => (
            execute_delegate_reject(&tc.function.arguments, board, from).await,
            false,
        ),
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
        "finish" => (execute_finish(&tc.function.arguments).await, true),
        _ => (format!("未知工具: {}", tc.function.name), false),
    }
}

async fn execute_finish(arguments: &str) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("(无摘要)");
    let result = args.get("result").and_then(|v| v.as_str()).unwrap_or("");
    format!("任务完成。摘要: {}\n结果: {}", summary, result)
}

async fn execute_delegate(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    parent_id: Option<&str>,
) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };

    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return "缺少 'target' 参数".to_string(),
    };
    let task = match args.get("task").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return "缺少 'task' 参数".to_string(),
    };
    let detail = args.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let priority = args.get("priority").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    match board
        .offer(from, target, task, detail, parent_id, priority)
        .await
    {
        Ok(task_id) => format!("委托已发布到发布区，task_id: {}", task_id),
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

async fn execute_delegate_reject(arguments: &str, board: &DelegationBoard, from: &str) -> String {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };
    let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return "缺少 'task_id' 参数".to_string(),
    };
    match board.reject(from, task_id).await {
        Ok(()) => format!("已驳回委托 {}, 任务回到发布区", task_id),
        Err(e) => format!("驳回失败: {}", e),
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
