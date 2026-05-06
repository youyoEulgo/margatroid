//! Member — 单个团队成员
//!
//! 封装模型、供应商、系统提示词、沙箱。
//! chat() 对外暴露干净的接口：prompt + 工具 → 结果 + 总结。
//! tool-call loop 和内联总结是私有实现，调用方不需要感知。

use anyhow::Result;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    ChatRequest, RequestMessage,
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::ResponseToolCall,
    RequestTool, ResponseChoice, FinishReason,
};

// ── DynProviderLike ──────────────────────────────────────────

/// 简化的 provider trait —— 避免泛型复杂度
#[async_trait::async_trait]
pub trait DynProviderLike: Send + Sync {
    async fn chat(
        &self,
        req: ChatRequest,
    ) -> Result<types::ChatResponse, providers::ProviderError>;
    fn id(&self) -> &'static str;
}

#[async_trait::async_trait]
impl<T: providers::AiProvider> DynProviderLike for T {
    async fn chat(
        &self,
        req: ChatRequest,
    ) -> Result<types::ChatResponse, providers::ProviderError> {
        providers::AiProvider::chat(self, req).await
    }
    fn id(&self) -> &'static str {
        providers::AiProvider::id(self)
    }
}

// ── Member ───────────────────────────────────────────────────

/// 单个团队成员
///
/// 持有自己的模型配置、供应商、和沙箱引用。
/// 不持有 worklog 或 board——这些是 Workspace 的职责。
pub struct Member {
    pub id: String,
    model: String,
    system_prompt: String,
    provider: Arc<dyn DynProviderLike>,
    sandbox: Arc<RwLock<SandboxManager>>,
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
        system_prompt: &str,
        provider: Arc<dyn DynProviderLike>,
        sandbox: Arc<RwLock<SandboxManager>>,
    ) -> Self {
        Self {
            id: id.to_string(),
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            provider,
            sandbox,
        }
    }

    /// 执行一次对话
    ///
    /// `prompt` 应由 Workspace 准备好，包含系统提示词、工作日志、相关记忆、当前任务。
    /// `tools` 由调用方决定注入哪些工具。
    ///
    /// 内部：tool-call loop → 结束后在同一上下文追加 summarization → 返回结果 + 总结。
    pub async fn chat(
        &self,
        prompt: &str,
        tools: &[RequestTool],
    ) -> Result<ChatOutcome> {
        let full_prompt = format!("{}\n\n---\n\n当前任务：{}", self.system_prompt, prompt);

        let mut messages: Vec<RequestMessage> = vec![RequestMessage::Chat(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(full_prompt),
            name: None,
            tool_calls: None,
        })];

        // ── 主 tool-call loop ──
        let final_content = loop {
            let req = ChatRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: if tools.is_empty() { None } else { Some(tools.to_vec()) },
                tool_choice: if tools.is_empty() {
                    None
                } else {
                    Some(types::RequestToolChoice::String("auto".into()))
                },
                ..Default::default()
            };

            let resp = self.provider.chat(req).await?;
            let choice = resp.choices.first().expect("no response choice");

            // 没有 tool calls → 主任务完成
            if choice.finish_reason == Some(FinishReason::Stop)
                && choice.message.tool_calls.as_ref().map_or(true, |v| v.is_empty())
            {
                break choice.message.content.clone().unwrap_or_default();
            }

            // 保存 assistant 消息（含 tool_calls）
            messages.push(request_message_from_choice(choice));

            // 执行 tool calls
            if let Some(tool_calls) = &choice.message.tool_calls {
                let sandbox_guard = self.sandbox.read().await;
                for tc in tool_calls {
                    let tool_result = execute_tool(tc, &sandbox_guard).await;
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

        let summary_resp = self.provider.chat(summary_req).await?;
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

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
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

async fn execute_tool(tc: &ResponseToolCall, sandbox: &SandboxManager) -> String {
    match tc.function.name.as_str() {
        "bash" => execute_bash(&tc.function.arguments, sandbox).await,
        "delegate" => format!("委托工具结果（由 Workspace 层面的循环处理）: {}", tc.function.arguments),
        _ => format!("未知工具: {}", tc.function.name),
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
    use types::ResponseMessage;

    struct NoopProvider;
    #[async_trait::async_trait]
    impl DynProviderLike for NoopProvider {
        async fn chat(&self, _req: ChatRequest) -> Result<types::ChatResponse, providers::ProviderError> {
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
        }
        fn id(&self) -> &'static str {
            "noop"
        }
    }

    #[tokio::test]
    async fn member_chat_without_tools() {
        let sandbox = Arc::new(RwLock::new(SandboxManager::new()));
        let member = Member::new(
            "test-agent",
            "noop",
            "你是测试助手",
            Arc::new(NoopProvider),
            sandbox,
        );

        let outcome = member
            .chat("工作日志: (暂无)\n\n回答: 1+1=?", &[])
            .await
            .unwrap();

        assert!(!outcome.result.is_empty());
        assert!(!outcome.summary.is_empty());
    }
}
