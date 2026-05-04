mod convert;
mod error;
mod types;

pub use error::OpenRouterError;

use self::types::{ApiErrorBody, WireResponse};
use crate::{error::ProviderError, traits::AiProvider};
use ::types::{ChatRequest, ChatResponse, StreamChunk};
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

    // 返回 ProviderError，? 在 impl AiProvider 里可以直接用
    async fn parse_response(&self, resp: reqwest::Response) -> Result<WireResponse, ProviderError> {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        if !status.is_success() {
            return match serde_json::from_str::<ApiErrorBody>(&body) {
                Ok(e) => Err(ProviderError::from(OpenRouterError::Api(e.error))),
                Err(_) => Err(ProviderError::ApiRaw {
                    status: status.as_u16(),
                    body,
                }),
            };
        }

        serde_json::from_str::<WireResponse>(&body).map_err(|e| ProviderError::Deserialize {
            message: e.to_string(),
            raw: body,
        })
    }

    fn parse_stream_line(line: &str) -> Option<Result<WireResponse, OpenRouterError>> {
        if line.starts_with(':') {
            return None;
        }
        let data = line.strip_prefix("data: ")?;
        if data.trim() == "[DONE]" {
            return None;
        }
        Some(
            serde_json::from_str::<WireResponse>(data).map_err(|e| OpenRouterError::StreamChunk {
                source: e,
                raw: data.to_owned(),
            }),
        )
    }
}

impl AiProvider for OpenRouterProvider {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let wire_req = convert::to_wire(&req);

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&wire_req)
            .send()
            .await
            // reqwest::Error → OpenRouterError::Http → ProviderError
            .map_err(|e| ProviderError::from(OpenRouterError::Http(e)))?;

        let wire_resp = self.parse_response(resp).await?;
        Ok(convert::from_wire(wire_resp))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>, ProviderError>
    {
        let mut wire_req = convert::to_wire(&req);
        wire_req.stream = Some(true);

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&wire_req)
            .send()
            .await
            .map_err(|e| ProviderError::from(OpenRouterError::Http(e)))?;

        // 非 2xx 在进入流之前就返回错误
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            return match serde_json::from_str::<ApiErrorBody>(&body) {
                Ok(e) => Err(ProviderError::from(OpenRouterError::Api(e.error))),
                Err(_) => Err(ProviderError::ApiRaw { status, body }),
            };
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buf = String::new();
            let mut byte_stream = byte_stream;

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(ProviderError::Network(e.to_string()));
                        return;
                    }
                };

                let text = match std::str::from_utf8(&chunk) {
                    Ok(t) => t,
                    Err(_) => {
                        yield Err(ProviderError::Deserialize {
                            message: "Invalid UTF-8 in stream".into(),
                            raw: String::new(),
                        });
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
                            Err(OpenRouterError::StreamChunk { source, raw }) => {
                                // chunk 解析失败，yield 错误但继续流
                                yield Err(ProviderError::StreamChunk {
                                    message: source.to_string(),
                                    raw,
                                });
                            }
                            Err(e) => {
                                // 其他错误中断流
                                yield Err(ProviderError::from(e));
                                return;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
