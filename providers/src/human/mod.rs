//! Human Provider — 将人类伪装为 AI 模型供应商
//!
//! 通过 HTTP 与 Human Server 通信：
//!   POST /api/human/request    创建请求
//!   GET  /api/human/request/{id} 阻塞等待回复（Notify 唤醒）
//!
//! chat() 创建请求后 GET 阻塞，人类回复时立即返回。

use crate::traits::AiProvider;
use anyhow::{Result, bail};
use types::{ChatRequest, ChatResponse};

pub struct HumanProvider {
    client: reqwest::Client,
    base_url: String,
}

impl HumanProvider {
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

impl AiProvider for HumanProvider {
    fn id(&self) -> &'static str {
        "human"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let tools = req.tools.unwrap_or_default();

        // 1. 创建请求
        let create_resp = self
            .client
            .post(format!("{}/api/human/request", self.base_url))
            .json(&serde_json::json!({
                "messages": req.messages,
                "tools": tools,
            }))
            .send()
            .await?;

        if !create_resp.status().is_success() {
            let status = create_resp.status();
            let body = create_resp.text().await.unwrap_or_default();
            bail!("human server returned {}: {}", status, body);
        }

        let body: serde_json::Value = create_resp.json().await?;
        let session_id = body["session_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("human server: missing session_id"))?
            .to_string();

        // 2. 阻塞等待回复（GET 挂起在 Notify 上，reply 来时立即返回）
        let url = format!("{}/api/human/request/{}", self.base_url, session_id);
        let resp = self.client.get(&url).send().await?;

        let body: serde_json::Value = resp.json().await?;
        match body["status"].as_str() {
            Some("completed") => {
                Ok(serde_json::from_value(body["response"].clone())?)
            }
            Some("timeout") => {
                bail!("human request timed out")
            }
            Some(s) => {
                bail!("human request unexpected status: {}", s)
            }
            None => {
                bail!("human request missing status")
            }
        }
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<types::StreamChunk>> + Send>>,
    > {
        bail!("human provider does not support streaming")
    }
}
