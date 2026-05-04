use crate::error::ProviderError;
use ::types::{ChatRequest, ChatResponse, StreamChunk};
use futures::Stream;
use std::pin::Pin;

/// 所有 AI provider 实现这个 trait
/// 上层代码（bridge / server）只依赖这个 trait，不依赖任何具体 provider
pub trait AiProvider: Send + Sync {
    /// provider 的唯一标识符，例如 "openrouter" / "anthropic"
    fn id(&self) -> &'static str;

    /// 非流式请求
    fn chat(
        &self,
        req: ChatRequest,
    ) -> impl Future<Output = Result<ChatResponse, ProviderError>> + Send;

    /// 流式请求
    /// 返回一个 Stream，每个 item 是一个 chunk 或错误
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

/// Object-safe version of `AiProvider` — uses boxed futures so it can be used as `dyn DynAiProvider`.
/// A blanket impl covers every `T: AiProvider`, so callers only implement `AiProvider`.
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
