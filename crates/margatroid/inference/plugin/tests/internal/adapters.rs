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
fn internal_response_has_no_format_specific_metadata() {
    let accumulator = ResponseAccumulator {
        content: "answer".into(),
        reasoning: "thought".into(),
        reason: Some(InferenceStopReason::Completed),
        ..Default::default()
    };
    let encoded = serde_json::to_value(accumulator.response().unwrap()).unwrap();
    assert_eq!(encoded["reasoning"], "thought");
    assert!(encoded.get("replay").is_none());
    assert!(encoded.get("api_format").is_none());
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
        assert!(adapter.encode(input.clone()).is_err());
    }
}

#[test]
fn domain_media_serializes_once_under_data_without_provider_fields() {
    let image = inference_core::InferenceImage {
        media_type: "image/png".into(),
        data: "bytes".into(),
    };
    for message in [
        InferenceMessage::User {
            content: "user".into(),
            images: vec![image.clone()],
        },
        InferenceMessage::Tool {
            call_id: "call".into(),
            content: "result".into(),
            images: vec![image],
            failed: false,
        },
    ] {
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["data"].as_array().unwrap().len(), 1);
        assert!(json.get("images").is_none());
        assert_eq!(
            serde_json::from_value::<InferenceMessage>(json).unwrap(),
            message
        );
    }
}
