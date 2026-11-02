use super::*;
use inference_core::InferenceParameters;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, Notify};

async fn http_server() -> (String, mpsc::Sender<String>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, mut receiver) = mpsc::channel::<String>(8);
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            if stream.read_exact(&mut byte).await.is_err() {
                return;
            }
            request.push(byte[0]);
        }
        let header = String::from_utf8_lossy(&request).to_ascii_lowercase();
        let length = header
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        if stream.read_exact(&mut body).await.is_err() {
            return;
        }
        if stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.is_err() { return; }
        while let Some(chunk) = receiver.recv().await {
            let frame = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
            if stream.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    });
    (url, sender, task)
}

fn chat(text: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})
    )
}

fn route(url: String) -> ModelRoute {
    ModelRoute {
        model: "model".into(),
        provider: "openai".into(),
        api_type: "chat_completions".into(),
        base_url: url,
        api_key: "test".into(),
        context_window_tokens: 1000,
        thinking: None,
        reasoning_effort: None,
    }
}

fn request() -> InferenceRequest {
    let mut world = World::new();
    InferenceRequest {
        id: "request".into(),
        agent: world.spawn(),
        turn: "turn".into(),
        messages: vec![inference_core::InferenceMessage::User {
            content: "hello".into(),
            images: vec![],
        }],
        tools: vec![],
        config: InferenceParameters {
            model: "route".into(),
            temperature: None,
            max_output_tokens: Some(100),
            top_p: None,
            stop: vec![],
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(None)),
    }
}

fn delivery() -> StreamDelivery {
    StreamDelivery {
        id: "request".into(),
        agent: "agent:test/agent:latest".into(),
        instance: "instance".into(),
        senders: vec![],
    }
}

#[tokio::test]
async fn real_http_backlog_is_discarded_without_entering_a_frozen_gate() {
    let (url, sender, server) = http_server().await;
    let input = DomainRequest {
        messages: request().messages,
        tools: vec![],
        config: request().config,
    };
    let outbound = build_request(input, &route(url)).unwrap();
    let cancel = CancellationToken::new();
    let (fragments, receiver) = mpsc::channel(2);
    let probe = fragments.clone();
    let worker = tokio::spawn(read_provider(
        reqwest::Client::new(),
        outbound,
        cancel.clone(),
        fragments,
    ));
    sender
        .send((0..64).map(|_| chat("not-accepted")).collect())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while probe.capacity() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let gate = Arc::new(Mutex::new(StreamGate::default()));
    gate.lock().unwrap().close();
    cancel.cancel();
    drop(probe);
    assert_eq!(
        receive_stream(gate.clone(), cancel, &mut delivery(), receiver)
            .await
            .unwrap_err()
            .kind,
        Kind::Cancelled
    );
    let _ = worker.await.unwrap();
    assert!(gate.lock().unwrap().accumulator.content.is_empty());
    assert_eq!(gate.lock().unwrap().sequence, 0);
    drop(sender);
    server.await.unwrap();
}

#[tokio::test]
async fn handle_cancel_freezes_synchronously_but_reply_waits_for_real_worker_join() {
    let (url, sender, server) = http_server().await;
    let request = request();
    let cancel = request.cancel.clone();
    let gate = Arc::new(Mutex::new(StreamGate::default()));
    let (reply, mut received) = oneshot::channel();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let context = InferenceContext {
        request,
        reply,
        route: route(url),
        tools: vec![],
        client: reqwest::Client::new(),
        gate: gate.clone(),
        delivery: delivery(),
        cleanup_gate: Some((entered.clone(), release.clone())),
    };
    let task = tokio::spawn(supervise_inference(context));
    let frozen = gate.clone();
    let handle = InferenceHandle {
        request_id: "request".into(),
        turn: "turn".into(),
        cancel,
        stop_stream: Arc::new(move || frozen.lock().unwrap().close()),
        task,
    };
    sender.send(chat("accepted")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while gate.lock().unwrap().accumulator.content != "accepted" {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    assert!(gate.lock().unwrap().closed);
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(matches!(
        received.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let _ = sender.send(chat("must-not-appear")).await;
    assert_eq!(gate.lock().unwrap().accumulator.content, "accepted");
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), handle.task)
        .await
        .unwrap()
        .unwrap();
    let response = received.await.unwrap().unwrap();
    assert_eq!(response.stop_reason, InferenceStopReason::Cancelled);
    assert_eq!(response.content.as_deref(), Some("accepted"));
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.replay, None);
    assert_eq!(response.usage, None);
    drop(sender);
    server.await.unwrap();
}

#[test]
fn chat_requires_finish_reason_and_done_and_rejects_data_after_terminal() {
    let terminal = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
    );
    for trace in [chat("text"), format!("{}data: [DONE]\n\n", chat("text"))] {
        let mut decoder = FormatDecoder::new(ApiFormat::ChatCompletions);
        let result = decoder
            .push(trace.as_bytes())
            .and_then(|_| decoder.finish());
        assert_eq!(result.err().unwrap().kind, Kind::ResponseIncomplete);
    }
    let mut decoder = FormatDecoder::new(ApiFormat::ChatCompletions);
    decoder
        .push(format!("{}{}", chat("text"), terminal).as_bytes())
        .unwrap();
    assert_eq!(
        decoder.push(chat("late").as_bytes()).err().unwrap().kind,
        Kind::ResponseInvalid
    );
}

#[test]
fn responses_completion_cannot_replace_the_streamed_text() {
    let events = [
        json!({"type":"response.created"}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","content":[]}}),
        json!({"type":"response.output_text.delta","output_index":0,"delta":"accepted"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"different"}]}}),
    ];
    let mut decoder = FormatDecoder::new(ApiFormat::Responses);
    for event in &events[..3] {
        decoder
            .push(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    assert_eq!(
        decoder
            .push(format!("data: {}\n\n", events[3]).as_bytes())
            .err()
            .unwrap()
            .kind,
        Kind::ResponseInvalid
    );
}

fn decode_events(
    format: ApiFormat,
    events: &[Value],
    tools: &[&str],
) -> Result<InferenceResponse, InferenceError> {
    let trace = events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect::<String>();
    let mut decoder = FormatDecoder::new(format);
    let mut accumulator = ResponseAccumulator::default();
    for byte in trace.as_bytes() {
        for fragment in decoder.push(&[*byte])? {
            accumulator.accept(fragment)?;
        }
    }
    for fragment in decoder.finish()? {
        accumulator.accept(fragment)?;
    }
    accumulator.response(&tools.iter().map(|name| name.to_string()).collect())
}

#[test]
fn messages_signed_reasoning_survives_byte_boundaries_and_replays_as_messages() {
    let events = vec![
        json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":2,"output_tokens":0,"cache_read_input_tokens":3}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"思考"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"answer"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
        json!({"type":"message_stop"}),
    ];
    let result = decode_events(ApiFormat::Messages, &events, &[]).unwrap();
    assert_eq!(result.reasoning.as_deref(), Some("思考"));
    assert_eq!(
        result.replay.as_ref().unwrap()["items"][0]["signature"],
        "signed"
    );
    assert_eq!(result.usage.as_ref().unwrap().input_tokens, 5);
    let mut input = DomainRequest {
        messages: request().messages,
        tools: vec![],
        config: request().config,
    };
    input
        .messages
        .push(inference_core::InferenceMessage::Assistant {
            reasoning: result.reasoning,
            content: result.content,
            tool_calls: result.tool_calls,
            replay: result.replay,
        });
    let mut selected = route("http://example.invalid/v1".into());
    selected.provider = "anthropic".into();
    selected.api_type = "messages".into();
    let encoded = build_request(input.clone(), &selected).unwrap();
    let body: Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(body["messages"][1]["content"][0]["signature"], "signed");
    selected.provider = "openai".into();
    selected.api_type = "responses".into();
    let converted = build_request(input, &selected).unwrap();
    let converted: Value = serde_json::from_slice(&converted.body).unwrap();
    assert!(converted.to_string().contains("answer"));
    assert!(!converted.to_string().contains("signed"));
    let mut unsigned = events;
    unsigned.remove(3);
    assert_eq!(
        decode_events(ApiFormat::Messages, &unsigned, &[])
            .unwrap_err()
            .kind,
        Kind::ResponseInvalid
    );
}

#[test]
fn responses_encrypted_reasoning_replay_preserves_full_local_context() {
    let events = vec![
        json!({"type":"response.created"}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"reason","summary":[],"encrypted_content":"encrypted"}}),
        json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"thought"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"reason","summary":[{"type":"summary_text","text":"thought"}]}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","content":[]}}),
        json!({"type":"response.output_text.delta","output_index":1,"delta":"answer"}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}}),
        json!({"type":"response.completed","response":{"status":"completed"}}),
    ];
    let result = decode_events(ApiFormat::Responses, &events, &[]).unwrap();
    assert_eq!(result.reasoning.as_deref(), Some("thought"));
    assert_eq!(
        result.replay.as_ref().unwrap()["items"][0]["encrypted_content"],
        "encrypted"
    );
    let mut input = DomainRequest {
        messages: request().messages,
        tools: vec![],
        config: request().config,
    };
    input
        .messages
        .push(inference_core::InferenceMessage::Assistant {
            reasoning: result.reasoning,
            content: result.content,
            tool_calls: result.tool_calls,
            replay: result.replay,
        });
    let mut selected = route("http://example.invalid/v1".into());
    selected.api_type = "responses".into();
    let body: Value =
        serde_json::from_slice(&build_request(input, &selected).unwrap().body).unwrap();
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][1]["encrypted_content"], "encrypted");
    assert_eq!(body["store"], false);
    assert!(body.get("previous_response_id").is_none());
}

#[test]
fn chat_tool_call_deltas_require_complete_arguments_and_visible_alias() {
    let trace = [
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"visible","arguments":"{\"x\":"}}]},"finish_reason":null}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}),
    ];
    let mut decoder = FormatDecoder::new(ApiFormat::ChatCompletions);
    let mut accumulator = ResponseAccumulator::default();
    for event in trace {
        for fragment in decoder
            .push(format!("data: {event}\n\n").as_bytes())
            .unwrap()
        {
            accumulator.accept(fragment).unwrap();
        }
    }
    for fragment in decoder.push(b"data: [DONE]\n\n").unwrap() {
        accumulator.accept(fragment).unwrap();
    }
    decoder.finish().unwrap();
    let result = accumulator
        .response(&HashSet::from(["visible".into()]))
        .unwrap();
    assert_eq!(result.tool_calls[0].arguments, json!({"x":1}));
    assert_eq!(result.stop_reason, InferenceStopReason::ToolCalls);
    assert_eq!(
        accumulator.response(&HashSet::new()).unwrap_err().kind,
        Kind::ResponseIncomplete
    );
}

#[test]
fn format_terminals_and_event_buffers_have_strict_bounds() {
    let events = [
        json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"open"}}),
        json!({"type":"message_stop"}),
    ];
    assert_eq!(
        decode_events(ApiFormat::Messages, &events, &[])
            .unwrap_err()
            .kind,
        Kind::ResponseIncomplete
    );
    let mut decoder = FormatDecoder::new(ApiFormat::Messages);
    let oversized = vec![b'x'; MAX_MESSAGE_BYTES + 1];
    assert_eq!(
        decoder.push(&oversized).err().unwrap().kind,
        Kind::ResponseTooLarge
    );
}

#[test]
fn chat_accepts_usage_with_repeated_empty_finished_choice_but_rejects_late_content() {
    let first = format!(
        "{}data: {}\n\n",
        chat("accepted"),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
    );
    let usage = json!({"choices":[{"index":0,"delta":{"content":"","role":"assistant"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
    let mut decoder = FormatDecoder::new(ApiFormat::ChatCompletions);
    let mut fragments = decoder.push(first.as_bytes()).unwrap();
    fragments.extend(
        decoder
            .push(format!("data: {usage}\n\ndata: [DONE]\n\n").as_bytes())
            .unwrap(),
    );
    fragments.extend(decoder.finish().unwrap());
    assert!(fragments.iter().any(|fragment| matches!(fragment, ProviderFragment::Usage(value) if value.input_tokens == 7 && value.output_tokens == 3)));
    assert_eq!(
        fragments
            .iter()
            .filter(|fragment| matches!(fragment, ProviderFragment::Content(_)))
            .count(),
        1
    );
    for choice in [
        json!({"index":0,"delta":{"content":"late"},"finish_reason":"stop"}),
        json!({"index":0,"delta":{},"finish_reason":"length"}),
        json!({"index":0,"delta":{"tool_calls":[{"index":0}]},"finish_reason":"stop"}),
    ] {
        let mut decoder = FormatDecoder::new(ApiFormat::ChatCompletions);
        decoder.push(first.as_bytes()).unwrap();
        assert_eq!(
            decoder
                .push(format!("data: {}\n\n", json!({"choices":[choice]})).as_bytes())
                .err()
                .unwrap()
                .kind,
            Kind::ResponseInvalid
        );
    }
}

#[tokio::test]
async fn gateway_that_accepts_but_sends_nothing_returns_timeout_and_releases_worker() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (release, wait) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let _ = socket.read(&mut request).await;
        let _ = wait.await;
    });
    let client = reqwest::Client::new();
    let outbound = build_request(
        DomainRequest {
            messages: request().messages,
            tools: vec![],
            config: request().config,
        },
        &route(url),
    )
    .unwrap();
    let (sender, _receiver) = mpsc::channel(2);
    let failed = tokio::time::timeout(
        Duration::from_secs(1),
        read_provider_with_timeouts(
            client,
            outbound,
            CancellationToken::new(),
            sender,
            (Duration::from_millis(40), Duration::from_millis(40)),
        ),
    )
    .await
    .unwrap()
    .err()
    .unwrap();
    assert_eq!(failed.kind, Kind::Timeout);
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn stalled_provider_body_is_a_timeout_instead_of_an_endless_receive() {
    let (url, chunks, server) = http_server().await;
    let client = reqwest::Client::new();
    let outbound = build_request(
        DomainRequest {
            messages: request().messages,
            tools: vec![],
            config: request().config,
        },
        &route(url),
    )
    .unwrap();
    let (sender, _receiver) = mpsc::channel(2);
    let failed = tokio::time::timeout(
        Duration::from_secs(1),
        read_provider_with_timeouts(
            client,
            outbound,
            CancellationToken::new(),
            sender,
            (Duration::from_millis(40), Duration::from_millis(40)),
        ),
    )
    .await
    .unwrap()
    .err()
    .unwrap();
    assert_eq!(failed.kind, Kind::Timeout);
    drop(chunks);
    server.await.unwrap();
}

#[tokio::test]
async fn response_header_deadline_is_independent_of_stream_idle_deadline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        let mut byte = [0];
        while !header.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
        }
        let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
        let length = header
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        socket.read_exact(&mut vec![0; length]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let body = format!(
            "{}data: {}\n\ndata: [DONE]\n\n",
            chat("delayed"),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
        );
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
    });
    let outbound = build_request(
        DomainRequest {
            messages: request().messages,
            tools: vec![],
            config: request().config,
        },
        &route(url),
    )
    .unwrap();
    let (sender, mut receiver) = mpsc::channel(8);
    read_provider_with_timeouts(
        reqwest::Client::new(),
        outbound,
        CancellationToken::new(),
        sender,
        (Duration::from_secs(1), Duration::from_millis(40)),
    )
    .await
    .unwrap();
    let mut fragments = vec![];
    while let Some(fragment) = receiver.recv().await {
        fragments.push(fragment);
    }
    assert!(fragments
        .iter()
        .any(|f| matches!(f,ProviderFragment::Content(text) if text=="delayed")));
    assert!(fragments
        .iter()
        .any(|f| matches!(f, ProviderFragment::Finished(_))));
    server.await.unwrap();
}
