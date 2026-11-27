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
fn chat_reasoning_is_decided_only_in_vendor_specialization() {
    let openai = adapter(Vendor::OpenAi, ApiFormat::ChatCompletions);
    let deepseek = adapter(Vendor::DeepSeek, ApiFormat::ChatCompletions);
    let openai_common = openai.encode(domain()).unwrap();
    let deepseek_common = deepseek.encode(domain()).unwrap();
    assert_eq!(
        serde_json::to_value(&openai_common).unwrap(),
        serde_json::to_value(&deepseek_common).unwrap()
    );
    let common = serde_json::to_value(&openai_common).unwrap();
    assert_eq!(common["messages"][1]["reasoning_content"], "local thought");
    let openai_http: Value =
        serde_json::from_slice(&openai.process_request(openai_common).unwrap().body).unwrap();
    let deepseek_http: Value =
        serde_json::from_slice(&deepseek.process_request(deepseek_common).unwrap().body).unwrap();
    assert!(openai_http["messages"][1]
        .get("reasoning_content")
        .is_none());
    assert_eq!(openai_http["messages"][1]["content"], "answer");
    assert_eq!(
        deepseek_http["messages"][1]["reasoning_content"],
        "local thought"
    );
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
fn all_common_formats_accept_reasoning_only_before_vendor_policy() {
    let mut input = domain();
    let InferenceMessage::Assistant { content, .. } = &mut input.messages[1] else {
        panic!()
    };
    *content = None;
    let original = input.messages.clone();
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
        let request = adapter
            .encode(input.clone())
            .expect("common conversion must not apply vendor rejection");
        match &request {
            ApiRequest::Responses(request) => assert!(
                matches!(&request.input[1],ResponsesInput::ReasoningOnly{reasoning,content}if reasoning=="local thought" && content.is_none())
            ),
            ApiRequest::ChatCompletions(request) => assert_eq!(
                serde_json::to_value(request).unwrap()["messages"][1]["reasoning_content"],
                "local thought"
            ),
            ApiRequest::Messages(request) => assert!(
                matches!(&request.messages[1],MessagesMessage::Assistant(assistant)if assistant.reasoning.as_deref()==Some("local thought") && assistant.content.is_empty())
            ),
        }
        assert_eq!(input.messages, original);
    }
}

#[test]
fn responses_vendor_projection_pads_openai_and_drops_deepseek_without_touching_domain() {
    for content in [None, Some(String::new())] {
        let mut input = domain();
        input.messages[1] = InferenceMessage::Assistant {
            reasoning: Some("interrupted private thought".into()),
            content,
            tool_calls: vec![],
        };
        input.messages.push(InferenceMessage::User {
            content: "continue".into(),
            images: vec![],
        });
        let original = input.messages.clone();
        let openai = adapter(Vendor::OpenAi, ApiFormat::Responses);
        let deepseek = adapter(Vendor::DeepSeek, ApiFormat::Responses);
        for (provider, expected) in [
            (
                &openai,
                json!([
                    {"role":"user","content":"prompt"},
                    {"role":"assistant","content":""},
                    {"role":"user","content":"continue"}
                ]),
            ),
            (
                &deepseek,
                json!([
                    {"role":"user","content":"prompt"},
                    {"role":"user","content":"continue"}
                ]),
            ),
            (
                &openai,
                json!([
                    {"role":"user","content":"prompt"},
                    {"role":"assistant","content":""},
                    {"role":"user","content":"continue"}
                ]),
            ),
        ] {
            let common = provider.encode(input.clone()).unwrap();
            let http = provider.process_request(common).unwrap();
            let body: Value = serde_json::from_slice(&http.body).unwrap();
            assert_eq!(body["input"], expected);
            assert_eq!(body["store"], false);
            assert!(!String::from_utf8(http.body)
                .unwrap()
                .contains("interrupted private thought"));
            assert_eq!(
                input.messages, original,
                "vendor switching must always start from original messages"
            );
        }
    }
}

#[test]
fn responses_keep_visible_text_and_tool_pairs_even_with_reasoning_or_empty_content() {
    let mut input = domain();
    input.messages.extend([
        InferenceMessage::Assistant {
            reasoning: Some("tool reasoning".into()),
            content: Some(String::new()),
            tool_calls: vec![inference_core::InferenceToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: json!({"path":"a.txt"}),
            }],
        },
        InferenceMessage::Tool {
            call_id: "call-1".into(),
            content: "tool output".into(),
            images: vec![],
            failed: false,
        },
        InferenceMessage::User {
            content: "continue".into(),
            images: vec![],
        },
    ]);
    let original = input.messages.clone();
    for vendor in [Vendor::OpenAi, Vendor::DeepSeek] {
        let provider = adapter(vendor, ApiFormat::Responses);
        let http = provider
            .process_request(provider.encode(input.clone()).unwrap())
            .unwrap();
        let body: Value = serde_json::from_slice(&http.body).unwrap();
        let items = body["input"].as_array().unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(
            items[1],
            json!({"role":"assistant","content":[{"type":"output_text","text":"answer"}]})
        );
        assert_eq!(
            items[2],
            json!({"type":"function_call","call_id":"call-1","name":"read_file","arguments":"{\"path\":\"a.txt\"}"})
        );
        assert_eq!(
            items[3],
            json!({"type":"function_call_output","call_id":"call-1","output":"tool output"})
        );
        assert!(!String::from_utf8(http.body)
            .unwrap()
            .contains("tool reasoning"));
        assert_eq!(input.messages, original);
    }
}

#[test]
fn empty_assistant_without_reasoning_is_still_invalid_domain_input() {
    let mut input = domain();
    input.messages[1] = InferenceMessage::Assistant {
        reasoning: None,
        content: Some(String::new()),
        tool_calls: vec![],
    };
    assert!(ResponsesRequest::try_from(input).is_err());
}

#[test]
fn other_vendor_rejections_are_after_common_conversion_not_domain_admission() {
    let mut input = domain();
    let InferenceMessage::Assistant { content, .. } = &mut input.messages[1] else {
        panic!()
    };
    *content = None;
    for (vendor, format) in [
        (Vendor::OpenAi, ApiFormat::ChatCompletions),
        (Vendor::Anthropic, ApiFormat::Messages),
        (Vendor::DeepSeek, ApiFormat::Messages),
    ] {
        let provider = adapter(vendor, format);
        let common = provider.encode(input.clone()).unwrap();
        assert!(provider.process_request(common).is_err());
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

#[test]
fn request_body_has_no_hidden_local_text_limit() {
    let route = ModelRoute {
        model: "model".into(),
        provider: "openai".into(),
        api_type: "responses".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        api_key: "key".into(),
        context_window_tokens: 1000,
        thinking: None,
        reasoning_effort: None,
    };
    let mut input = domain();
    input.messages[0] = InferenceMessage::User {
        content: "x".repeat(17 * 1024 * 1024),
        images: vec![],
    };
    assert!(build_request(input.clone(), &route).unwrap().body.len() > 17 * 1024 * 1024);
}
