//! AI Provider 通用类型
//!
//! DynAiProvider —— object-safe trait，runtime 通过它调用 LLM
//! 统一使用 anyhow::Error 处理错误

use crate::{ChatRequest, ChatResponse, StreamChunk};
use anyhow::Result;
use futures::Stream;
use std::pin::Pin;

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe AI provider trait。
///
/// runtime 持有 `Arc<dyn DynAiProvider>`，通过它做 LLM 调用，
/// 完全不需要知道具体 provider 实现。
pub trait DynAiProvider: Send + Sync {
    fn id(&self) -> &'static str;

    fn chat_boxed(&self, req: ChatRequest) -> ProviderFuture<'_, Result<ChatResponse>>;

    fn chat_stream_boxed(&self, req: ChatRequest) -> ProviderFuture<'_, Result<ProviderStream>>;
}

/// 具体 provider 实现此 trait，使用 `impl Future` 返回类型（非 object-safe）。
///
/// blanket impl 自动将其转换为 `DynAiProvider`。
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &'static str;

    fn chat(&self, req: ChatRequest) -> impl Future<Output = Result<ChatResponse>> + Send;

    fn chat_stream(&self, req: ChatRequest) -> impl Future<Output = Result<ProviderStream>> + Send;
}

// AiProvider → DynAiProvider 的 blanket impl
impl<T: AiProvider> DynAiProvider for T {
    fn id(&self) -> &'static str {
        AiProvider::id(self)
    }

    fn chat_boxed(&self, req: ChatRequest) -> ProviderFuture<'_, Result<ChatResponse>> {
        Box::pin(AiProvider::chat(self, req))
    }

    fn chat_stream_boxed(&self, req: ChatRequest) -> ProviderFuture<'_, Result<ProviderStream>> {
        Box::pin(AiProvider::chat_stream(self, req))
    }
}
