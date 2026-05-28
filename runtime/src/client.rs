//! AI 客户端 — 封装模型名称与供应商，统一聊天接口

use anyhow::Result;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::Arc;
use types::{
    ChatRequest, ChatResponse, DynAiProvider, RequestMessage, RequestTool, message::MessageContent,
};

pub struct Client {
    model: String,
    provider: Arc<dyn DynAiProvider>,
    verbose: bool,
}

impl Client {
    pub fn new(model: String, provider: Arc<dyn DynAiProvider>, verbose: bool) -> Self {
        Self {
            model,
            provider,
            verbose,
        }
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose
    }

    pub async fn chat(
        &self,
        messages: Vec<RequestMessage>,
        tools: &[RequestTool],
    ) -> Result<ChatResponse> {
        let req = ChatRequest {
            model: self.model.clone(),
            messages,
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

        if self.verbose {
            verbose_request(&self.model, &req.messages, tools);
        }
        tracing::debug!(
            "raw request: {}",
            serde_json::to_string_pretty(&req).unwrap_or_default(),
        );

        let resp = self.provider.chat_boxed(req).await?;

        if self.verbose {
            verbose_response(&self.model, &resp);
        }
        tracing::debug!(
            "raw response: {}",
            serde_json::to_string_pretty(&resp).unwrap_or_default(),
        );

        Ok(resp)
    }

    /// 流式请求（降级：不支持流式时回退到非流式，包装为单条 chunk）
    pub async fn chat_stream(
        &self,
        messages: Vec<RequestMessage>,
        tools: &[RequestTool],
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<String>> + Send>>> {
        let tools_vec = if tools.is_empty() {
            None
        } else {
            Some(tools.to_vec())
        };

        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.clone(),
            stream: Some(true),
            tools: tools_vec.clone(),
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(types::RequestToolChoice::String("auto".into()))
            },
            ..Default::default()
        };

        if self.verbose {
            verbose_request(&self.model, &req.messages, tools);
        }
        tracing::debug!(
            "raw request: {}",
            serde_json::to_string_pretty(&req).unwrap_or_default(),
        );

        match self.provider.chat_stream_boxed(req).await {
            Ok(stream) => {
                let s = stream.map(|chunk| Ok(serde_json::to_string(&chunk?)?));
                Ok(Box::pin(s))
            }
            Err(e) => {
                tracing::warn!("stream failed, falling back: {}", e);
                let req2 = ChatRequest {
                    model: self.model.clone(),
                    messages,
                    stream: Some(false),
                    tools: tools_vec,
                    tool_choice: if tools.is_empty() {
                        None
                    } else {
                        Some(types::RequestToolChoice::String("auto".into()))
                    },
                    ..Default::default()
                };
                let resp = self.provider.chat_boxed(req2).await?;
                let json = serde_json::to_string(&resp)?;

                if self.verbose {
                    verbose_response(&self.model, &resp);
                }
                tracing::debug!("raw fallback response: {}", json);

                Ok(Box::pin(futures::stream::once(async { Ok(json) })))
            }
        }
    }
}

// ── verbose 输出（tracing::info!，需 --verbose 开启） ──

fn verbose_request(model: &str, messages: &[RequestMessage], tools: &[RequestTool]) {
    let mut out = String::new();
    for m in messages {
        match m {
            RequestMessage::Chat(c) => {
                let content = match &c.content {
                    MessageContent::Text(t) => truncate(t, 140),
                    _ => "(multipart)".into(),
                };
                out.push_str(&format!(
                    "  [{:>9}] {}\n",
                    format!("{:?}", c.role).to_lowercase(),
                    content,
                ));
            }
            RequestMessage::Tool(t) => {
                out.push_str(&format!(
                    "  [    tool] {} ← {}\n",
                    truncate(&t.content, 80),
                    &t.tool_call_id[..t.tool_call_id.len().min(8)],
                ));
            }
        }
    }
    out.push_str("  --- tools ---\n");
    for t in tools {
        out.push_str(&format!("  {}\n", t.function.name));
    }
    tracing::info!(
        "→ LLM | model={} | {} msgs:\n{}",
        model,
        messages.len(),
        out,
    );
}

fn verbose_response(model: &str, resp: &ChatResponse) {
    if let Some(c) = resp.choices.first() {
        let text = c.message.content.as_deref().unwrap_or("(none)");
        let tokens = resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
        let mut out = format!(
            "← LLM | model={} | tokens={} | finish={:?}\n",
            model, tokens, c.finish_reason,
        );
        out.push_str(&format!("  text: {}\n", truncate(text, 200)));
        if let Some(tcs) = &c.message.tool_calls {
            out.push_str("  tool_calls:\n");
            for tc in tcs {
                out.push_str(&format!(
                    "    {}({})\n",
                    tc.function.name,
                    verbose_args(&tc.function.arguments)
                ));
            }
        }
        tracing::info!("{}", out);
    }
}

fn verbose_args(json: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return truncate(json, 80),
    };
    match v {
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => format!("\"{}\"", truncate(&s, 60)),
                    other => other.to_string(),
                };
                format!("{}: {}", k, val)
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => truncate(json, 80),
    }
}

pub fn verbose_stream_done(text: &str, tool_calls: &str) {
    let mut out = format!("stream done | text_len={}", text.len());
    if !text.is_empty() {
        let s = text.replace('\n', "\\n");
        let preview = if s.len() > 200 {
            let cut: String = s
                .char_indices()
                .take_while(|(i, _)| *i < 200)
                .map(|(_, c)| c)
                .collect();
            format!("{}...", cut)
        } else {
            s
        };
        out.push_str(&format!("\n  text: {}", preview));
    }
    if !tool_calls.is_empty() {
        out.push_str(&format!("\n  tool_calls: {}", tool_calls));
    }
    tracing::info!("{}", out);
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', "\\n");
    if s.len() <= max {
        return s;
    }
    let cut = s
        .char_indices()
        .take_while(|(i, _)| *i < max)
        .map(|(_, c)| c)
        .collect::<String>();
    format!("{}...", cut)
}
