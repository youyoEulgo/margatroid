//! Member V2 — 团队成员
//!
//! 持有 EventBus，chat() 中直接发送事件到全局通道。
//! 移除了对 board 事件方法的依赖。

use anyhow::Result;
use futures::StreamExt;
use sandbox::SandboxManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{
    message::{ChatMessage, MessageContent, Role, ToolMessage},
    tool::ResponseToolCall,
    FinishReason, Identity, RequestMessage, RequestTool, ResponseChoice,
};

use crate::board::DelegationBoard;
use crate::events::EventBus;

// 使用 providers crate 的 Client
use providers::Client;

/// 所有成员（LLM 或 Human）都实现这个 trait
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    fn id(&self) -> &str;
    fn identity(&self) -> &Identity;

    /// 处理一个任务：组装上下文 → LLM tool-call loop → 结果
    async fn process(
        &self,
        board: &DelegationBoard,
        tools: &[RequestTool],
        system_prompt: &str,
        member_profiles: &[types::MemberProfile],
    ) -> Result<ChatOutcome>;
}

/// chat() 返回值
pub struct ChatOutcome {
    pub result: String,
}

// ── Member ───────────────────────────────────────────────────

pub struct Member {
    pub id: String,
    soul: String,
    identity: Identity,
    client: Client,
    sandbox: Arc<RwLock<SandboxManager>>,
    event_bus: Arc<EventBus>,
    workspace_name: String,
}

impl Member {
    pub fn new(
        id: &str,
        soul: String,
        identity: Identity,
        client: Client,
        sandbox: Arc<RwLock<SandboxManager>>,
        event_bus: Arc<EventBus>,
        workspace_name: String,
    ) -> Self {
        Self {
            id: id.to_string(),
            soul,
            identity,
            client,
            sandbox,
            event_bus,
            workspace_name,
        }
    }

    /// 发送事件到 workspace 统一事件流
    fn send_event(
        &self,
        event_name: &str,
        delegation_id: &str,
        content: types::events::EventContent,
    ) {
        let payload = types::events::EventPayload::new(event_name, &self.id, delegation_id);
        let event = types::events::WorkspaceEvent { payload, content };
        if let Ok(json) = serde_json::to_string(&event) {
            let channel_name = format!("{}/stream", self.workspace_name);
            let _ = self.event_bus.send(&channel_name, json);
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

    async fn process(
        &self,
        board: &DelegationBoard,
        tools: &[RequestTool],
        system_prompt: &str,
        member_profiles: &[types::MemberProfile],
    ) -> Result<ChatOutcome> {
        self.chat(board, tools, system_prompt, member_profiles)
            .await
    }
}

const LOOP_CONSTRAINT: &str = "你必须完成当前委托或发布新委托才能结束";

impl Member {
    async fn chat(
        &self,
        board: &DelegationBoard,
        tools: &[RequestTool],
        system_prompt: &str,
        member_profiles: &[types::MemberProfile],
    ) -> Result<ChatOutcome> {
        // 组装上下文
        let chain = board.chain_snapshot().await;
        let worklog = crate::context::format_worklog(board.db());
        let memories = format_memories(board.db(), &self.id);
        let chat_messages = crate::context::assemble_prompt(
            system_prompt,
            member_profiles,
            &worklog,
            &chain,
            &self.soul,
            &memories,
        );

        let mut messages: Vec<RequestMessage> = chat_messages
            .into_iter()
            .map(RequestMessage::Chat)
            .collect();

        let did = chain
            .current_task()
            .map(|t| t.id.clone())
            .unwrap_or_default();

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

                // 解析 chunk
                let chunk: types::StreamChunk =
                    serde_json::from_str(&chunk_json).unwrap_or_else(|_| types::StreamChunk {
                        id: String::new(),
                        model: String::new(),
                        choices: vec![],
                        usage: None,
                    });

                // 发送 stream_chunk 事件
                self.send_event(
                    types::event_index::EVENT_STREAM_CHUNK,
                    &did,
                    types::events::EventContent::StreamChunk {
                        chunk: chunk.clone(),
                    },
                );

                if chunk.choices.is_empty() {
                    // 降级处理：完整响应
                    if let Ok(resp) = serde_json::from_str::<types::ChatResponse>(&chunk_json) {
                        if let Some(choice) = resp.choices.first() {
                            full_content = choice.message.content.clone().unwrap_or_default();
                            full_reasoning =
                                choice.message.reasoning_content.clone().unwrap_or_default();
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

            // Debug: 打印最终累积的 tool_calls
            for tc in &full_tool_calls {
                tracing::debug!(
                    "final tool_call | id={} | name={} | args_len={}",
                    tc.id,
                    tc.function.name,
                    tc.function.arguments.len()
                );
            }

            // verbose 日志：流式摘要
            if self.client.is_verbose() {
                let tc_str = full_tool_calls
                    .iter()
                    .map(|tc| {
                        format!(
                            "{}({})",
                            tc.function.name,
                            format_args_json(&tc.function.arguments)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                providers::verbose_stream_done(&full_content, &tc_str);
            }

            // 保存完整文本到 DB
            if !full_content.is_empty() {
                save_conversation(board, &self.id, &full_content).await;
            }

            // 构造伪 ResponseChoice
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

            // 如果只返回文本，添加约束消息继续循环
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

                for tc in &full_tool_calls {
                    let tr = crate::tools::execute_tool(
                        &tc.function.name,
                        &tc.function.arguments,
                        &sandbox_guard,
                        board,
                        &self.id,
                        &full_content,
                    )
                    .await;

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
                    // 发送 chain_update 事件（任务完成，链左移）
                    let chain = board.chain_snapshot().await;
                    let (from, to, brief) = chain
                        .current_task()
                        .map_or((String::new(), String::new(), String::new()), |t| {
                            (t.from.clone(), t.to.clone(), t.brief.clone())
                        });
                    let head_pos = chain.head;
                    self.send_event(
                        types::event_index::EVENT_CHAIN_UPDATE,
                        &did,
                        types::events::EventContent::ChainUpdate {
                            from,
                            to,
                            brief,
                            head_pos,
                        },
                    );
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

fn format_memories(db: &crate::memory::SqliteMemory, member_id: &str) -> String {
    let my_log = match db.worklog_by_agent(member_id, 10) {
        Ok(log) => log,
        Err(e) => {
            tracing::warn!("无法查询成员 '{}' 的工作日志: {}", member_id, e);
            return String::new();
        }
    };
    let delegation_ids: Vec<String> = my_log.iter().map(|e| e.delegation_id.clone()).collect();
    let memories = db.personal_by_delegations(&delegation_ids);
    if memories.is_empty() {
        return String::new();
    }
    memories
        .iter()
        .map(|m| {
            let short_id = &m.delegation_id[..m.delegation_id.len().min(8)];
            format!("[{}] {} — tags: {}", short_id, m.summary, m.tags.join(", "))
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

fn merge_deltas(full: &mut Vec<ResponseToolCall>, deltas: &[types::tool::ToolCallDelta]) {
    for delta in deltas {
        let idx = delta.index as usize;

        // 自动填充空白占位符，确保 full[idx] 存在
        while full.len() <= idx {
            full.push(ResponseToolCall {
                id: String::new(),
                r#type: "function".into(),
                function: types::tool::ResponseFunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
        }

        let existing = &mut full[idx];

        // 累积 id（如果有）
        if let Some(id) = &delta.id {
            if !id.is_empty() {
                existing.id.clone_from(id);
            }
        }

        // 累积 type（如果有）
        if let Some(t) = &delta.r#type {
            existing.r#type.clone_from(t);
        }

        // 累积 function 信息
        if let Some(ref f) = delta.function {
            if let Some(ref name) = f.name {
                if !name.is_empty() {
                    existing.function.name.clone_from(name);
                }
            }
            if let Some(ref args) = f.arguments {
                existing.function.arguments.push_str(args);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_deltas_new_call() {
        let mut full = vec![];
        let deltas = vec![types::tool::ToolCallDelta {
            index: 0,
            id: Some("call1".to_string()),
            r#type: Some("function".to_string()),
            function: Some(types::tool::FunctionCallDelta {
                name: Some("test".to_string()),
                arguments: Some("{\"a\":".to_string()),
            }),
        }];

        merge_deltas(&mut full, &deltas);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].id, "call1");
        assert_eq!(full[0].function.name, "test");
        assert_eq!(full[0].function.arguments, "{\"a\":");
    }

    #[test]
    fn test_merge_deltas_accumulate() {
        let mut full = vec![ResponseToolCall {
            id: "call1".to_string(),
            r#type: "function".to_string(),
            function: types::tool::ResponseFunctionCall {
                name: "test".to_string(),
                arguments: "{\"a\":".to_string(),
            },
        }];

        let deltas = vec![types::tool::ToolCallDelta {
            index: 0,
            id: Some("call1".to_string()),
            r#type: None,
            function: Some(types::tool::FunctionCallDelta {
                name: None,
                arguments: Some("1}".to_string()),
            }),
        }];

        merge_deltas(&mut full, &deltas);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].function.arguments, "{\"a\":1}");
    }
}
