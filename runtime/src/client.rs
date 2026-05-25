//! AI 客户端 — 封装模型名称与供应商，统一聊天接口

use anyhow::Result;
use std::sync::Arc;
use types::{ChatRequest, ChatResponse, DynAiProvider, RequestMessage, RequestTool};

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
            tracing::info!(
                "[DEBUG] → LLM request | model={} | messages={} | tools={}",
                self.model,
                serde_json::to_string_pretty(&req.messages).unwrap_or_default(),
                serde_json::to_string_pretty(&req.tools).unwrap_or_default(),
            );
        }

        let resp = self.provider.chat_boxed(req).await?;

        if self.verbose {
            tracing::info!(
                "[DEBUG] ← LLM response | model={} | {}",
                self.model,
                serde_json::to_string_pretty(&resp).unwrap_or_default(),
            );
        }

        Ok(resp)
    }
}
