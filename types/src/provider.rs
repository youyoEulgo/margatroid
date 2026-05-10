//! AI Provider 通用类型
//!
//! ProviderError —— 统一的 provider 错误类型
//! DynAiProvider —— object-safe trait，runtime 通过它调用 LLM

use crate::{ChatRequest, ChatResponse, StreamChunk};
use futures::Stream;
use std::pin::Pin;

/// 对上层暴露的统一 provider 错误
#[derive(Debug)]
pub enum ProviderError {
    /// 网络层错误（连接超时、DNS 失败等）
    Network(String),

    /// API 返回了错误状态码，并携带了结构化的错误信息
    Api {
        code: i32,
        message: String,
        /// provider 原始错误元数据，透传给调用方
        metadata: Option<serde_json::Value>,
    },

    /// API 返回了错误状态码，但响应体无法解析
    ApiRaw { status: u16, body: String },

    /// 响应体反序列化失败
    Deserialize { message: String, raw: String },

    /// 流式响应中单个 chunk 解析失败
    StreamChunk { message: String, raw: String },

    /// 请求参数非法（在发出请求之前就可以检测到）
    InvalidRequest(String),

    /// provider 不支持请求的功能
    Unsupported(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Api { code, message, .. } => write!(f, "API error {code}: {message}"),
            Self::ApiRaw { status, body } => write!(f, "API error (HTTP {status}): {body}"),
            Self::Deserialize { message, raw } => {
                write!(f, "Deserialize error: {message}; raw: {raw}")
            }
            Self::StreamChunk { message, raw } => {
                write!(f, "Stream chunk error: {message}; chunk: {raw}")
            }
            Self::InvalidRequest(msg) => write!(f, "Invalid request: {msg}"),
            Self::Unsupported(msg) => write!(f, "Unsupported: {msg}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Object-safe AI provider trait。
///
/// runtime 持有 `Arc<dyn DynAiProvider>`，通过它做 LLM 调用，
/// 完全不需要知道具体 provider 实现。
pub trait DynAiProvider: Send + Sync {
    fn id(&self) -> &'static str;

    fn chat_boxed(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>;

    fn chat_stream_boxed(
        &self,
        req: ChatRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    >;
}

/// 具体 provider 实现此 trait，使用 `impl Future` 返回类型（非 object-safe）。
///
/// blanket impl 自动将其转换为 `DynAiProvider`。
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &'static str;

    fn chat(
        &self,
        req: ChatRequest,
    ) -> impl Future<Output = Result<ChatResponse, ProviderError>> + Send;

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> impl Future<
        Output = Result<
            Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>,
            ProviderError,
        >,
    > + Send;
}

// AiProvider → DynAiProvider 的 blanket impl
impl<T: AiProvider> DynAiProvider for T {
    fn id(&self) -> &'static str {
        AiProvider::id(self)
    }

    fn chat_boxed(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        Box::pin(AiProvider::chat(self, req))
    }

    fn chat_stream_boxed(
        &self,
        req: ChatRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(AiProvider::chat_stream(self, req))
    }
}
