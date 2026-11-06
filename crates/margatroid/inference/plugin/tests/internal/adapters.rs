use super::*;
use inference_core::InferenceParameters;

fn domain() -> DomainRequest {
    DomainRequest {
        messages: vec![
            InferenceMessage::User {
                content: "prompt".into(),
                images: vec![],
            },
            InferenceMessage::Assistant {
                reasoning: Some("local thought".into()),
                replay: None,
                content: Some("answer".into()),
                tool_calls: vec![],
            },
        ],
        tools: vec![],
        config: InferenceParameters {
            model: "model".into(),
            max_output_tokens: Some(128),
            temperature: None,
            top_p: None,
            stop: vec![],
        },
    }
}

fn adapter(vendor: Vendor, format: ApiFormat) -> ApiAdapter {
    build_adapter(
        vendor,
        format,
        ProviderRouteInput {
            base_url: &Url::parse("http://localhost/v1").unwrap(),
            api_key: "key",
            thinking: None,
            reasoning_effort: None,
        },
    )
    .unwrap()
}

#[test]
fn supported_matrix_has_concrete_handlers_and_bound_decoders() {
    for (vendor, format) in [
        (Vendor::OpenAi, ApiFormat::ChatCompletions),
        (Vendor::OpenAi, ApiFormat::Responses),
        (Vendor::Anthropic, ApiFormat::Messages),
        (Vendor::DeepSeek, ApiFormat::ChatCompletions),
        (Vendor::DeepSeek, ApiFormat::Messages),
        (Vendor::DeepSeek, ApiFormat::Responses),
        (Vendor::OpenRouter, ApiFormat::ChatCompletions),
    ] {
        let adapter = adapter(vendor, format);
        match &adapter {
            ApiAdapter::OpenAiChat(handler) => {
                let _: &OpenAi<ChatCompletionsRequest> = handler;
            }
            ApiAdapter::OpenAiResponses(handler) => {
                let _: &OpenAi<ResponsesRequest> = handler;
            }
            ApiAdapter::AnthropicMessages(handler) => {
                let _: &Anthropic<MessagesRequest> = handler;
            }
            ApiAdapter::DeepSeekChat(handler) => {
                let _: &DeepSeek<ChatCompletionsRequest> = handler;
            }
            ApiAdapter::DeepSeekMessages(handler) => {
                let _: &DeepSeek<MessagesRequest> = handler;
            }
            ApiAdapter::DeepSeekResponses(handler) => {
                let _: &DeepSeek<ResponsesRequest> = handler;
            }
            ApiAdapter::OpenRouterChat(handler) => {
                let _: &OpenRouter<ChatCompletionsRequest> = handler;
            }
        }
        let encoded = adapter.encode(domain()).unwrap();
        let outbound = adapter.process_request(encoded).unwrap();
        assert_eq!(outbound.decoder.format, format);
    }
    for (vendor, format) in [
        (Vendor::OpenAi, ApiFormat::Messages),
        (Vendor::Anthropic, ApiFormat::ChatCompletions),
        (Vendor::Anthropic, ApiFormat::Responses),
        (Vendor::OpenRouter, ApiFormat::Messages),
        (Vendor::OpenRouter, ApiFormat::Responses),
    ] {
        assert!(build_adapter(
            vendor,
            format,
            ProviderRouteInput {
                base_url: &Url::parse("http://localhost/v1").unwrap(),
                api_key: "key",
                thinking: None,
                reasoning_effort: None,
            }
        )
        .is_err());
    }
}

#[test]
fn chat_reasoning_is_decided_during_format_conversion() {
    let openai = adapter(Vendor::OpenAi, ApiFormat::ChatCompletions);
    let ApiRequest::ChatCompletions(request) = openai.encode(domain()).unwrap() else {
        panic!()
    };
    assert!(request.messages[1].get("reasoning_content").is_none());
    assert_eq!(request.messages[1]["content"], "answer");
    let deepseek = adapter(Vendor::DeepSeek, ApiFormat::ChatCompletions);
    let ApiRequest::ChatCompletions(request) = deepseek.encode(domain()).unwrap() else {
        panic!()
    };
    assert_eq!(request.messages[1]["reasoning_content"], "local thought");
    assert!(openai
        .process_request(ApiRequest::Responses(domain().try_into().unwrap()))
        .is_err());
}

#[test]
fn historical_replay_is_optional_but_current_response_replay_is_strict() {
    assert!(replay_items(None, ApiFormat::Messages).unwrap().is_empty());
    let replay = json!({"api_format":"messages","items":[{"type":"thinking","thinking":"thought","signature":"signed","extra":"preserved"}]});
    assert_eq!(
        replay_items(Some(replay.clone()), ApiFormat::Messages).unwrap(),
        replay["items"].as_array().unwrap().clone()
    );
    assert!(replay_items(Some(replay), ApiFormat::Responses)
        .unwrap()
        .is_empty());
    assert!(replay_items(
        Some(json!({"api_format":"messages","items":[{"type":"thinking","thinking":"unsigned"}]})),
        ApiFormat::Responses
    )
    .is_err());
    assert!(replay_items(
        Some(json!({"api_format":"unknown","items":[]})),
        ApiFormat::Responses
    )
    .is_err());
    let accumulator = ResponseAccumulator {
        content: "answer".into(),
        reasoning: "thought".into(),
        replay_format: Some("messages".into()),
        reason: Some(InferenceStopReason::Completed),
        ..Default::default()
    };
    assert_eq!(
        accumulator.response().unwrap_err().kind,
        InferenceErrorKind::ResponseInvalid
    );
}

#[test]
fn reasoning_only_history_does_not_require_fabricated_thinking() {
    let mut input = domain();
    let InferenceMessage::Assistant { content, .. } = &mut input.messages[1] else {
        panic!()
    };
    *content = None;
    for (vendor, format) in [
        (Vendor::Anthropic, ApiFormat::Messages),
        (Vendor::OpenAi, ApiFormat::Responses),
        (Vendor::OpenAi, ApiFormat::ChatCompletions),
    ] {
        let adapter = adapter(vendor, format);
        let encoded = adapter.encode(input.clone()).unwrap();
        let body = adapter.process_request(encoded).unwrap().body;
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert!(!body.to_string().contains("local thought"));
    }
}
