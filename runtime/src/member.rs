//! Member — 单个团队成员
//!
//! 封装模型、供应商、沙箱、委托板。
//! chat() 对外暴露干净的接口：prompt + 工具 → 结果 + 总结。
//! 委托（delegate/delegate_async）通过 self.board 发布，LLM 自行决策阻塞/非阻塞。

use anyhow::Result;
use providers::DynAiProvider;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    ChatRequest, FinishReason, RequestMessage, RequestTool, ResponseChoice,
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::ResponseToolCall,
};

use crate::board::DelegationBoard;

// ── Member ───────────────────────────────────────────────────

/// 单个团队成员
pub struct Member {
    pub id: String,
    model: String,
    provider: Arc<dyn DynAiProvider>,
    sandbox: Arc<RwLock<SandboxManager>>,
    board: Arc<DelegationBoard>,
}

/// chat() 返回值
pub struct ChatOutcome {
    /// 最终回复文本（给委托方的结果）
    pub result: String,
    /// 内联总结（同一 KV cache 上下文生成，用于记忆）
    pub summary: String,
}

impl Member {
    pub fn new(
        id: &str,
        model: &str,
        provider: Arc<dyn DynAiProvider>,
        sandbox: Arc<RwLock<SandboxManager>>,
        board: Arc<DelegationBoard>,
    ) -> Self {
        Self {
            id: id.to_string(),
            model: model.to_string(),
            provider,
            sandbox,
            board,
        }
    }

    /// 执行一次对话
    ///
    /// `prompt` 应由 Workspace 准备好，包含系统提示词、工作日志、相关记忆、当前任务。
    /// `tools` 由调用方决定注入哪些工具。
    ///
    /// 内部：tool-call loop → 结束后在同一上下文追加 summarization → 返回结果 + 总结。
    pub async fn chat(&self, prompt: &str, tools: &[RequestTool]) -> Result<ChatOutcome> {
        let mut messages: Vec<RequestMessage> = vec![RequestMessage::Chat(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(prompt.to_string()),
            name: None,
            tool_calls: None,
        })];

        // ── 主 tool-call loop ──
        let final_content = loop {
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

            // 没有 tool calls → 主任务完成
            if choice.finish_reason == Some(FinishReason::Stop)
                && choice
                    .message
                    .tool_calls
                    .as_ref()
                    .map_or(true, |v| v.is_empty())
            {
                break choice.message.content.clone().unwrap_or_default();
            }

            // 保存 assistant 消息（含 tool_calls）
            messages.push(request_message_from_choice(choice));

            // 执行 tool calls
            if let Some(tool_calls) = &choice.message.tool_calls {
                let sandbox_guard = self.sandbox.read().await;
                for tc in tool_calls {
                    let tool_result = execute_tool(tc, &sandbox_guard, &self.board, &self.id).await;
                    messages.push(RequestMessage::Tool(ToolMessage {
                        role: Role::Tool,
                        content: tool_result,
                        tool_call_id: tc.id.clone(),
                        name: None,
                    }));
                }
                drop(sandbox_guard);
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
            .unwrap_or("(summary unavailable)")
            .to_string();

        Ok(ChatOutcome {
            result: final_content,
            summary,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
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
) -> String {
    match tc.function.name.as_str() {
        "bash" => execute_bash(&tc.function.arguments, sandbox).await,
        "delegate" => execute_delegate(&tc.function.arguments, board, from).await,
        "schedule_add" => execute_schedule_add(&tc.function.arguments, board).await,
        "schedule_list" => execute_schedule_list(board).await,
        "schedule_pop" => execute_schedule_pop(&tc.function.arguments, board).await,
        "schedule_remove" => execute_schedule_remove(&tc.function.arguments, board).await,
        _ => format!("未知工具: {}", tc.function.name),
    }
}

async fn execute_delegate(arguments: &str, board: &DelegationBoard, from: &str) -> String {
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

    match board
        .offer(from, target, task, serde_json::json!({}), 1)
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::Stream;
    use std::{future::Future, pin::Pin};
    use types::ResponseMessage;

    struct NoopProvider;
    impl DynAiProvider for NoopProvider {
        fn chat_boxed(
            &self,
            _req: ChatRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<types::ChatResponse, providers::ProviderError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(types::ChatResponse {
                    id: "noop".into(),
                    model: "noop".into(),
                    created: 0,
                    choices: vec![ResponseChoice {
                        index: 0,
                        message: ResponseMessage {
                            role: "assistant".into(),
                            content: Some("OK".into()),
                            tool_calls: None,
                        },
                        finish_reason: Some(FinishReason::Stop),
                    }],
                    usage: None,
                })
            })
        }

        fn chat_stream_boxed(
            &self,
            _req: ChatRequest,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Pin<
                                Box<
                                    dyn Stream<
                                            Item = Result<
                                                types::StreamChunk,
                                                providers::ProviderError,
                                            >,
                                        > + Send,
                                >,
                            >,
                            providers::ProviderError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Err(providers::ProviderError::Unsupported(
                    "streaming not supported in tests".into(),
                ))
            })
        }

        fn id(&self) -> &'static str {
            "noop"
        }
    }

    #[tokio::test]
    async fn member_chat_without_tools() {
        let sandbox = Arc::new(RwLock::new(SandboxManager::new()));
        let board = Arc::new(DelegationBoard::new(Arc::new(
            crate::memory::SqliteMemory::open(":memory:").unwrap(),
        )));
        let member = Member::new("test-agent", "noop", Arc::new(NoopProvider), sandbox, board);

        let outcome = member
            .chat(
                "你是测试助手\n\n---\n\n工作日志: (暂无)\n\n回答: 1+1=?",
                &[],
            )
            .await
            .unwrap();

        assert!(!outcome.result.is_empty());
        assert!(!outcome.summary.is_empty());
    }
}
