//! AI 客户端 — 封装模型名称与供应商，统一聊天接口

use anyhow::Result;
use std::sync::Arc;
use types::{ChatRequest, ChatResponse, DynAiProvider, RequestMessage, RequestTool};

pub struct Client {
    model: String,
    provider: Arc<dyn DynAiProvider>,
}

impl Client {
    pub fn new(model: String, provider: Arc<dyn DynAiProvider>) -> Self {
        Self { model, provider }
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
        self.provider.chat_boxed(req).await
    }
}
