mod convert;
mod types;

use self::types::{ApiErrorBody, WireResponse};
use crate::traits::AiProvider;
use ::types::{ChatRequest, ChatResponse, StreamChunk};
use anyhow::{Context, Result, bail};
use futures::{Stream, StreamExt};
use std::pin::Pin;

const BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouterProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: BASE_URL.into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    // 返回 anyhow::Result，? 在 impl AiProvider 里可以直接用
    async fn parse_response(&self, resp: reqwest::Response) -> Result<WireResponse> {
        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;
        if !status.is_success() {
            let msg = serde_json::from_str::<ApiErrorBody>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            bail!("API error (HTTP {}): {}", status.as_u16(), msg);
        }
        serde_json::from_str(&body).context("failed to deserialize response")
    }

    fn parse_stream_line(line: &str) -> Option<Result<WireResponse, serde_json::Error>> {
        if line.starts_with(':') {
            return None;
        }
        let data = line.strip_prefix("data: ")?;
        if data.trim() == "[DONE]" {
            return None;
        }
        Some(serde_json::from_str(data))
    }
}

impl AiProvider for OpenRouterProvider {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let wire_req = convert::to_wire(&req);

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&wire_req)
            .send()
            .await
            .context("HTTP request failed")?;

        let wire_resp = self.parse_response(resp).await?;
        Ok(convert::from_wire(wire_resp))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let mut wire_req = convert::to_wire(&req);
        wire_req.stream = Some(true);

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&wire_req)
            .send()
            .await
            .context("HTTP request failed")?;

        // 非 2xx 在进入流之前就返回错误
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.context("failed to read response body")?;
            let msg = serde_json::from_str::<ApiErrorBody>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            bail!("API error (HTTP {}): {}", status, msg);
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buf = String::new();
            let mut byte_stream = byte_stream;

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(anyhow::anyhow!("network error: {}", e));
                        return;
                    }
                };

                let text = match std::str::from_utf8(&chunk) {
                    Ok(t) => t,
                    Err(_) => {
                        yield Err(anyhow::anyhow!("invalid UTF-8 in stream"));
                        return;
                    }
                };

                buf.push_str(text);

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_owned();
                    buf = buf[pos + 1..].to_owned();

                    if line.is_empty() {
                        continue;
                    }

                    if let Some(result) = Self::parse_stream_line(&line) {
                        match result {
                            Ok(wire) => yield Ok(convert::from_wire_stream(wire)),
                            Err(e) => {
                                yield Err(anyhow::anyhow!("chunk parse error: {}", e));
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
