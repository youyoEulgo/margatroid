//! Member — 单个团队成员 + Agent trait
//!
//! 封装客户端、沙箱。
//! Agent trait 定义统一调用接口，Member 是唯一实现。
//! chat() 驱动 LLM tool-call loop，通过 Board.assemble_prompt() 获取上下文。

use anyhow::Result;
use futures::StreamExt;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    FinishReason, Identity, RequestMessage, RequestTool, ResponseChoice,
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::{ResponseFunctionCall, ResponseToolCall, ToolCallDelta},
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

/// tool 执行结果
struct ToolResult {
    content: String,
    should_break: bool,
    is_error: bool,
}

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

        let did = current_delegation_id(board).await;

        // 主 tool-call loop
        let final_content = loop {
            let mut stream = self.client.chat_stream(messages.clone(), tools).await?;

            let mut full_content = String::new();
            let mut full_reasoning = String::new();
            let mut full_tool_calls: Vec<ResponseToolCall> = Vec::new();
            let mut finish_reason: Option<FinishReason> = None;

            while let Some(chunk_result) = stream.next().await {
                let chunk_json = match chunk_result {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("stream chunk error, skipping: {}", e);
                        continue;
                    }
                };
                tracing::debug!("raw chunk: {}", chunk_json);
                board.publish_raw(&did, &chunk_json).await;

                let chunk: types::StreamChunk =
                    serde_json::from_str(&chunk_json).unwrap_or_else(|_| {
                        // 可能是一整个 ChatResponse（降级路径）
                        types::StreamChunk {
                            id: String::new(),
                            model: String::new(),
                            choices: vec![],
                            usage: None,
                        }
                    });

                if chunk.choices.is_empty() {
                    // ChatResponse 降级：解析为完整响应
                    if let Ok(resp) = serde_json::from_str::<types::ChatResponse>(&chunk_json) {
                        if let Some(choice) = resp.choices.first() {
                            full_content = choice.message.content.clone().unwrap_or_default();
                            full_reasoning = choice.message.reasoning_content.clone().unwrap_or_default();
                            full_tool_calls = choice.message.tool_calls.clone().unwrap_or_default();
                            finish_reason = choice.finish_reason.clone();
                        }
                    }
                    continue;
                }

                if let Some(choice) = chunk.choices.first() {
                    if let Some(content) = &choice.delta.content {
                        full_content.push_str(content);
                    }
                    if let Some(r) = &choice.delta.reasoning_content {
                        full_reasoning.push_str(r);
                    }
                    if let Some(tcs) = &choice.delta.tool_calls {
                        merge_deltas(&mut full_tool_calls, tcs);
                    }
                    if finish_reason.is_none() {
                        finish_reason.clone_from(&choice.finish_reason);
                    }
                }
            }

            tracing::debug!(
                "stream ended | fr={:?} | text_len={} | tool_count={}",
                finish_reason,
                full_content.len(),
                full_tool_calls.len(),
            );

            // 保存完整文本到 DB
            if !full_content.is_empty() {
                save_conversation(board, &self.id, &full_content).await;
            }

            // info: 接收流量的工具摘要
            let used: Vec<&str> = full_tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect();
            tracing::info!(
                "← LLM | {} | used=[{}]",
                self.id,
                used.join(", "),
            );

            // verbose 日志：流式摘要
            if self.client.is_verbose() {
                let tc_str = full_tool_calls
                    .iter()
                    .map(|tc| format!("{}({})", tc.function.name, format_args_json(&tc.function.arguments)))
                    .collect::<Vec<_>>()
                    .join(", ");
                crate::client::verbose_stream_done(&full_content, &tc_str);
            }

            // 构造伪 ResponseChoice 沿用后续逻辑
            let fr = finish_reason.clone();
            let choice = ResponseChoice {
                index: 0,
                message: types::ResponseMessage {
                    role: "assistant".into(),
                    content: if full_content.is_empty() {
                        None
                    } else {
                        Some(full_content.clone())
                    },
                    tool_calls: if full_tool_calls.is_empty() {
                        None
                    } else {
                        Some(full_tool_calls.clone())
                    },
                    reasoning_content: if full_reasoning.is_empty() {
                        None
                    } else {
                        Some(full_reasoning.clone())
                    },
                },
                finish_reason,
            };

            if fr == Some(FinishReason::Stop) && full_tool_calls.is_empty() {
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(full_content),
                    name: None,
                    tool_calls: None,
                    reasoning_content: if full_reasoning.is_empty() {
                        None
                    } else {
                        Some(full_reasoning.clone())
                    },
                }));
                messages.push(RequestMessage::Chat(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(LOOP_CONSTRAINT.to_string()),
                    name: None,
                    tool_calls: None,
                    reasoning_content: None,
                }));
                continue;
            }

            messages.push(request_message_from_choice(&choice));

            if !full_tool_calls.is_empty() {
                let sandbox_guard = self.sandbox.read().await;
                let mut should_break = false;
                let mut break_content = String::new();
                let reply = &full_content;

                for tc in &full_tool_calls {
                    let tr =
                        execute_tool(tc, &sandbox_guard, board, &self.id, reply).await;

                    messages.push(RequestMessage::Tool(ToolMessage {
                        role: Role::Tool,
                        content: tr.content.clone(),
                        tool_call_id: tc.id.clone(),
                        name: None,
                    }));

                    if tr.should_break && !tr.is_error {
                        should_break = true;
                        break_content = tr.content;
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
        reasoning_content: choice.message.reasoning_content.clone(),
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
        "bash" => ToolResult {
            content: execute_bash(&tc.function.arguments, sandbox).await,
            should_break: false,
            is_error: false,
        },
        "delegate" => {
            let mut tr =
                execute_delegate(&tc.function.arguments, board, from, reply).await;
            tr.should_break = true;
            tr
        }
        "schedule_add" => ToolResult {
            content: execute_schedule_add(&tc.function.arguments, board).await,
            should_break: false,
            is_error: false,
        },
        "schedule_list" => ToolResult {
            content: execute_schedule_list(board).await,
            should_break: false,
            is_error: false,
        },
        "schedule_pop" => ToolResult {
            content: execute_schedule_pop(&tc.function.arguments, board).await,
            should_break: false,
            is_error: false,
        },
        "schedule_remove" => ToolResult {
            content: execute_schedule_remove(&tc.function.arguments, board).await,
            should_break: false,
            is_error: false,
        },
        "recall" => ToolResult {
            content: execute_recall(&tc.function.arguments, board).await,
            should_break: false,
            is_error: false,
        },
        "finish" => execute_finish(&tc.function.arguments, board, from, reply).await,
        _ => ToolResult {
            content: format!("未知工具: {}", tc.function.name),
            should_break: false,
            is_error: false,
        },
    }
}

async fn execute_finish(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolResult {
    let args: serde_json::Value = if arguments.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        }
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
            let short_did = &did[..did.len().min(8)];
            tracing::info!(
                "finish | {} ← {} | did={}",
                task_from,
                from,
                short_did,
            );
            board.publish_raw(&did, r#"{"type":"done"}"#).await;
            ToolResult {
                content: format!("完成。摘要: {}", summary),
                should_break: true,
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("产出写入失败: {}", e),
            should_break: false,
            is_error: true,
        },
    }
}

async fn execute_delegate(
    arguments: &str,
    board: &DelegationBoard,
    from: &str,
    reply: &str,
) -> ToolResult {
    let args: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return ToolResult {
            content: format!("参数解析失败: {}", e),
            should_break: false,
            is_error: true,
        },
    };
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return ToolResult {
            content: "缺少 'target' 参数".to_string(),
            should_break: false,
            is_error: true,
        },
    };
    let task_summary = match args.get("task_summary").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return ToolResult {
            content: "缺少 'task_summary' 参数".to_string(),
            should_break: false,
            is_error: true,
        },
    };
    let task_detail = match args.get("task_detail").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return ToolResult {
            content: "缺少 'task_detail' 参数".to_string(),
            should_break: false,
            is_error: true,
        },
    };
    let work_summary = match args.get("work_summary").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult {
            content: "缺少 'work_summary' 参数".to_string(),
            should_break: false,
            is_error: true,
        },
    };
    let work_detail = match args.get("work_detail").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return ToolResult {
            content: "缺少 'work_detail' 参数".to_string(),
            should_break: false,
            is_error: true,
        },
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

    board.publish_raw(&parent_id.clone().unwrap_or_default(), r#"{"type":"done"}"#).await;

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
        Ok(task_id) => ToolResult {
            content: format!(
                "委托已发布到发布区，task_id: {}\n发委托前总结: {}\n发委托前思路: {}",
                task_id, work_summary, work_detail
            ),
            should_break: false,
            is_error: false,
        },
        Err(e) => ToolResult {
            content: format!("委托发布失败: {}", e),
            should_break: false,
            is_error: true,
        },
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

async fn current_delegation_id(board: &DelegationBoard) -> String {
    board
        .chain_snapshot()
        .await
        .current_task()
        .map(|t| t.id.clone())
        .unwrap_or_default()
}

fn merge_deltas(accum: &mut Vec<ResponseToolCall>, deltas: &[ToolCallDelta]) {
    for td in deltas {
        let idx = td.index as usize;
        while accum.len() <= idx {
            accum.push(ResponseToolCall {
                id: String::new(),
                r#type: "function".into(),
                function: ResponseFunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
        }
        let existing = &mut accum[idx];
        if let Some(id) = &td.id {
            if !id.is_empty() {
                existing.id.clone_from(id);
            }
        }
        if let Some(t) = &td.r#type {
            existing.r#type.clone_from(t);
        }
        if let Some(f) = &td.function {
            if let Some(name) = &f.name {
                if !name.is_empty() {
                    existing.function.name.clone_from(name);
                }
            }
            if let Some(args) = &f.arguments {
                existing.function.arguments.push_str(args);
            }
        }
    }
}

fn format_args_json(json: &str) -> String {
    if json.is_empty() {
        return "(empty)".into();
    }
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => {
            let cut: String = json
                .char_indices()
                .take_while(|(i, _)| *i < 80)
                .map(|(_, c)| c)
                .collect();
            return if json.len() > 80 {
                format!("{}...", cut)
            } else {
                cut
            };
        }
    };
    match v {
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => {
                        let cut: String = s
                            .char_indices()
                            .take_while(|(i, _)| *i < 60)
                            .map(|(_, c)| c)
                            .collect();
                        if s.len() > 60 {
                            format!("\"{}...\"", cut)
                        } else {
                            format!("\"{}\"", cut)
                        }
                    }
                    other => other.to_string(),
                };
                format!("{}: {}", k, val)
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => {
            let cut: String = json
                .char_indices()
                .take_while(|(i, _)| *i < 80)
                .map(|(_, c)| c)
                .collect();
            if json.len() > 80 {
                format!("{}...", cut)
            } else {
                cut
            }
        }
    }
}
