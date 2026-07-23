use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_runtime_plugin::{AsyncAppExt, AsyncRuntimePlugin, AsyncSystemOptions};
use core_plugin::{App, Stage, World};
use providers::{AiProvider, OpenRouterProvider};
use types::message::{ChatMessage, MessageContent, RequestMessage, Role};
use types::ChatRequest;

#[derive(Clone)]
struct LlmRequest {
    prompt: String,
}

#[derive(Clone, Debug)]
enum LlmOutcome {
    Completed(String),
    Failed(String),
}

#[test]
#[ignore = "requires MARGATROID_LLM_URL, MARGATROID_LLM_API_KEY and MARGATROID_LLM_MODEL"]
fn real_llm_runs_through_async_system_and_returns_an_event() {
    let base_url = required_env("MARGATROID_LLM_URL");
    let api_key = required_env("MARGATROID_LLM_API_KEY");
    let model = required_env("MARGATROID_LLM_MODEL");

    let provider = Arc::new(OpenRouterProvider::new(api_key).with_base_url(base_url));
    let mut app = App::new();
    app.add_plugins(AsyncRuntimePlugin::default());
    app.add_async_system_with_options(
        move |request: LlmRequest| {
            let provider = provider.clone();
            let model = model.clone();
            async move {
                let messages = vec![RequestMessage::Chat(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(request.prompt),
                    name: None,
                    tool_calls: None,
                    reasoning_content: None,
                })];
                let request = ChatRequest {
                    model,
                    messages,
                    stream: Some(false),
                    max_tokens: Some(256),
                    ..Default::default()
                };
                let mut last_error = String::new();
                for attempt in 0..5 {
                    match provider.chat(request.clone()).await {
                        Ok(response) => {
                            return LlmOutcome::Completed(
                                response
                                    .choices
                                    .first()
                                    .and_then(|choice| choice.message.content.clone())
                                    .unwrap_or_default(),
                            );
                        }
                        Err(error) => last_error = error.to_string(),
                    }
                    tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
                }
                LlmOutcome::Failed(last_error)
            }
        },
        AsyncSystemOptions::with_timeout(Duration::from_secs(60)),
    );

    let mut request = Some(LlmRequest {
        prompt: "Reply with exactly: ASYNC_RUNTIME_PLUGIN_LLM_OK".into(),
    });
    app.add_systems(
        Stage::Update,
        [move |world: &mut World| {
            if let Some(request) = request.take() {
                world.send_event(request);
            }
        }],
    );

    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let system_outcomes = outcomes.clone();
    let mut reader = app.event_reader::<LlmOutcome>();
    app.add_systems(
        Stage::Update,
        [move |world: &mut World| {
            system_outcomes
                .lock()
                .unwrap()
                .extend(world.read_events(&mut reader));
        }],
    );

    let deadline = Instant::now() + Duration::from_secs(65);
    while outcomes.lock().unwrap().is_empty() && Instant::now() < deadline {
        app.tick();
        std::thread::sleep(Duration::from_millis(10));
    }

    let outcome = outcomes.lock().unwrap().first().cloned();
    match outcome {
        Some(LlmOutcome::Completed(content)) => assert!(
            content.contains("ASYNC_RUNTIME_PLUGIN_LLM_OK"),
            "unexpected model response: {content:?}"
        ),
        Some(LlmOutcome::Failed(error)) => panic!("LLM request failed: {error}"),
        None => panic!("LLM request did not finish before timeout"),
    }
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing required environment variable {name}"))
}
