use futures::StreamExt;

use crate::events::{LlmFailed, LlmFailureKind, LlmRequest, LlmResponse, LlmStreamChunk};
use crate::resource::LlmProviderRegistry;

#[derive(Clone, Debug)]
pub struct LlmAsyncBatch {
    pub outputs: Vec<LlmAsyncOutput>,
}

#[derive(Clone, Debug)]
pub enum LlmAsyncOutput {
    Response(LlmResponse),
    StreamChunk(LlmStreamChunk),
    Failed(LlmFailed),
}

pub(crate) async fn handle_llm_request(
    registry: LlmProviderRegistry,
    request: LlmRequest,
) -> LlmAsyncBatch {
    let Some(provider) = registry.get(&request.provider) else {
        return LlmAsyncBatch::single(LlmAsyncOutput::Failed(LlmFailed {
            request_id: request.request_id,
            provider: request.provider,
            kind: LlmFailureKind::ProviderNotFound,
            message: "provider is not registered".into(),
        }));
    };

    if request.request.stream.unwrap_or(false) {
        match provider.chat_stream_boxed(request.request).await {
            Ok(mut stream) => {
                let mut outputs = Vec::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => {
                            outputs.push(LlmAsyncOutput::StreamChunk(LlmStreamChunk {
                                request_id: request.request_id.clone(),
                                chunk,
                            }));
                        }
                        Err(error) => {
                            return LlmAsyncBatch::single(LlmAsyncOutput::Failed(LlmFailed {
                                request_id: request.request_id,
                                provider: request.provider,
                                kind: LlmFailureKind::StreamFailed,
                                message: error.to_string(),
                            }));
                        }
                    }
                }
                if outputs.is_empty() {
                    LlmAsyncBatch::single(LlmAsyncOutput::Failed(LlmFailed {
                        request_id: request.request_id,
                        provider: request.provider,
                        kind: LlmFailureKind::StreamFailed,
                        message: "provider stream ended without chunks".into(),
                    }))
                } else {
                    LlmAsyncBatch { outputs }
                }
            }
            Err(error) => LlmAsyncBatch::single(LlmAsyncOutput::Failed(LlmFailed {
                request_id: request.request_id,
                provider: request.provider,
                kind: LlmFailureKind::RequestFailed,
                message: error.to_string(),
            })),
        }
    } else {
        match provider.chat_boxed(request.request).await {
            Ok(response) => LlmAsyncBatch::single(LlmAsyncOutput::Response(LlmResponse {
                request_id: request.request_id,
                response,
            })),
            Err(error) => LlmAsyncBatch::single(LlmAsyncOutput::Failed(LlmFailed {
                request_id: request.request_id,
                provider: request.provider,
                kind: LlmFailureKind::RequestFailed,
                message: error.to_string(),
            })),
        }
    }
}

impl LlmAsyncBatch {
    fn single(output: LlmAsyncOutput) -> Self {
        Self {
            outputs: vec![output],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use anyhow::Result;
    use core_plugin::{App, Stage, World};
    use futures::{stream, Stream};
    use types::{
        ChatRequest, ChatResponse, DynAiProvider, ResponseChoice, ResponseMessage, StreamChunk,
    };

    use crate::{LlmFailed, LlmPlugin, LlmProviderRegistry, LlmRequest, LlmResponse};

    struct FakeProvider;

    impl DynAiProvider for FakeProvider {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn chat_boxed(
            &self,
            _req: ChatRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ChatResponse>> + Send + '_>> {
            Box::pin(async {
                Ok(ChatResponse {
                    id: "fake-response".into(),
                    model: "fake".into(),
                    choices: vec![ResponseChoice {
                        index: 0,
                        message: ResponseMessage {
                            role: "assistant".into(),
                            content: Some("ok".into()),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                    created: 0,
                })
            })
        }

        fn chat_stream_boxed(
            &self,
            _req: ChatRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(Box::pin(stream::iter(Vec::<Result<StreamChunk>>::new()))
                    as Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>)
            })
        }
    }

    #[test]
    fn plugin_routes_chat_request_to_provider() {
        let mut app = App::new();
        app.add_plugins(LlmPlugin::new());
        app.world()
            .resource::<LlmProviderRegistry>()
            .unwrap()
            .register("fake", Arc::new(FakeProvider))
            .unwrap();

        let responses = Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_responses = responses.clone();
        let mut reader = app.event_reader::<LlmResponse>();
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                system_responses
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world()
            .send_event(LlmRequest::new("req-1", "fake", ChatRequest::default()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while responses.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_eq!(responses.lock().unwrap().len(), 1);
    }

    #[test]
    fn plugin_reports_missing_provider() {
        let mut app = App::new();
        app.add_plugins(LlmPlugin::new());

        let failures = Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_failures = failures.clone();
        let mut reader = app.event_reader::<LlmFailed>();
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                system_failures
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world()
            .send_event(LlmRequest::new("req-1", "missing", ChatRequest::default()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while failures.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_eq!(failures.lock().unwrap().len(), 1);
    }
}
