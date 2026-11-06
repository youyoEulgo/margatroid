use super::*;
#[path = "tool_contract.rs"]
mod tool_contract;
use inference_core::InferenceParameters;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, Notify};

async fn http_server() -> (String, mpsc::Sender<String>, tokio::task::JoinHandle<()>) {
    let (url, sender, task, _) = http_server_with_request().await;
    (url, sender, task)
}

async fn http_server_with_request() -> (
    String,
    mpsc::Sender<String>,
    tokio::task::JoinHandle<()>,
    oneshot::Receiver<Value>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, mut receiver) = mpsc::channel::<String>(8);
    let (captured, request_body) = oneshot::channel();
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
        let _ = captured.send(serde_json::from_slice(&body).unwrap());
        if stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.is_err() { return; }
        while let Some(chunk) = receiver.recv().await {
            let frame = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
            if stream.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    });
    (url, sender, task, request_body)
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
        reply: Some(reply),
        route: route(url),
        tools: vec![],
        client: reqwest::Client::new(),
        gate: gate.clone(),
        delivery: delivery(),
        cleanup_notification: None,
        cleanup_gate: Some((entered.clone(), release.clone())),
    };
    let task = tokio::spawn(supervise_inference(context));
    let frozen = gate.clone();
    let handle = InferenceHandle {
        request_id: "request".into(),
        turn: "turn".into(),
        cancel,
        stop_stream: Arc::new(move || frozen.lock().unwrap().close()),
        task: Arc::new(inference_core::InferenceTaskState::default()),
    };
    handle.attach_task(task).unwrap();
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
    tokio::time::timeout(Duration::from_secs(2), handle.wait_finished())
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

#[tokio::test]
async fn provider_error_replies_before_stalled_cleanup_and_retains_active_handle() {
    let (url, sender, server) = http_server().await;
    let input = request();
    let cancel = input.cancel.clone();
    let gate = Arc::new(Mutex::new(StreamGate::default()));
    let frozen = gate.clone();
    let (reply, received) = oneshot::channel();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let task = tokio::spawn(supervise_inference(InferenceContext {
        request: input,
        reply: Some(reply),
        route: route(url),
        tools: vec![],
        client: reqwest::Client::new(),
        gate: gate.clone(),
        delivery: delivery(),
        cleanup_notification: None,
        cleanup_gate: Some((entered.clone(), release.clone())),
    }));
    let handle = InferenceHandle {
        request_id: "request".into(),
        turn: "turn".into(),
        cancel,
        stop_stream: Arc::new(move || frozen.lock().unwrap().close()),
        task: Arc::new(inference_core::InferenceTaskState::default()),
    };
    handle.attach_task(task).unwrap();
    sender.send("data: invalid-json\n\n".into()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(1), received)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(failed.kind, Kind::ResponseInvalid);
    assert!(gate.lock().unwrap().closed);
    assert!(
        !handle.is_finished(),
        "reply must retain actual cleanup supervision"
    );
    let mut world = World::new();
    let entity = world.spawn();
    world.insert_component(
        entity,
        Agent {
            name: "test".into(),
            workspace: entity,
            state: AgentState::Working,
            turn: Some("turn".into()),
        },
    );
    world.insert_component(
        entity,
        ActiveInference {
            current: Some(handle),
        },
    );
    let mut next = request();
    next.agent = entity;
    let (reply, rejected) = oneshot::channel();
    next.reply = Arc::new(Mutex::new(Some(reply)));
    handle_inference(&mut world, next);
    assert_eq!(
        rejected.await.unwrap().unwrap_err().kind,
        Kind::InferenceBusy
    );
    let handle = world
        .get_component_mut::<ActiveInference>(entity)
        .unwrap()
        .current
        .take()
        .unwrap();
    assert!(!handle.is_finished());
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), handle.wait_finished())
        .await
        .unwrap()
        .unwrap();
    drop(sender);
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_cleanup_budget_replies_error_and_keeps_real_join_active() {
    let (url, sender, server) = http_server().await;
    let input = request();
    let cancel = input.cancel.clone();
    let gate = Arc::new(Mutex::new(StreamGate::default()));
    let frozen = gate.clone();
    let (reply, received) = oneshot::channel();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let task = tokio::spawn(supervise_inference(InferenceContext {
        request: input,
        reply: Some(reply),
        route: route(url),
        tools: vec![],
        client: reqwest::Client::new(),
        gate: gate.clone(),
        delivery: delivery(),
        cleanup_notification: None,
        cleanup_gate: Some((entered.clone(), release.clone())),
    }));
    let handle = InferenceHandle {
        request_id: "request".into(),
        turn: "turn".into(),
        cancel,
        stop_stream: Arc::new(move || frozen.lock().unwrap().close()),
        task: Arc::new(inference_core::InferenceTaskState::default()),
    };
    handle.attach_task(task).unwrap();
    sender.send(chat("accepted")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while gate.lock().unwrap().accumulator.content != "accepted" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(4), received)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(failed.kind, Kind::CleanupFailed);
    assert!(
        !handle.is_finished(),
        "cleanup timeout must not pretend the worker joined"
    );
    assert!(gate.lock().unwrap().closed);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), handle.wait_finished())
        .await
        .unwrap()
        .unwrap();
    drop(sender);
    server.await.unwrap();
}

#[tokio::test]
async fn real_cleanup_wakes_matching_driver_only_after_supervisor_finishes() {
    use app_runtime_plugin::RuntimePlugin;
    use core_plugin::App;
    use lua_runtime_core::{DriverLuaVmInstance, DriverWakeSlot};

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    app.add_system(RuntimePlugin::UPDATE, crate::system::inference_system);
    let agent = app.world_mut().spawn();
    let instance = DriverLuaVmInstance {
        owner: agent,
        start_id: "current-instance".into(),
    };
    let wake = DriverWakeSlot::shared();
    app.world_mut().insert_component(
        agent,
        DriverLuaVm {
            instance: instance.clone(),
            wake: wake.clone(),
            ready: Arc::new(Mutex::new(None)),
            ended: None,
            cancel: CancellationToken::new(),
        },
    );
    let cleanup = InferenceCleanupFinished {
        agent,
        request_id: "request".into(),
        instance,
    };
    let (url, sender, server) = http_server().await;
    let mut input = request();
    input.agent = agent;
    let cancel = input.cancel.clone();
    let (reply, received) = oneshot::channel();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let task = tokio::spawn(supervise_inference(InferenceContext {
        request: input,
        reply: Some(reply),
        route: route(url),
        tools: vec![],
        client: reqwest::Client::new(),
        gate: Arc::new(Mutex::new(StreamGate::default())),
        delivery: delivery(),
        cleanup_notification: Some((
            InferenceCleanupSender::Runtime(app.world().event_sender()),
            cleanup.clone(),
        )),
        cleanup_gate: Some((entered.clone(), release.clone())),
    }));
    app.world_mut().insert_component(
        agent,
        ActiveInference {
            current: Some(attach_handle(
                InferenceHandle {
                    request_id: "request".into(),
                    turn: "turn".into(),
                    cancel,
                    stop_stream: Arc::new(|| {}),
                    task: Arc::new(inference_core::InferenceTaskState::default()),
                },
                task,
            )),
        },
    );
    sender.send("data: invalid-json\n\n".into()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        Kind::ResponseInvalid
    );
    assert_eq!(app.world().event_snapshot().normal_event_count, 0);
    let mut waiting = Box::pin(DriverWakeSlot::wait(wake.clone()));
    assert!(waiting.as_mut().now_or_never().is_none());

    // The actual supervisor must not emit its completion event while cleanup
    // remains blocked, even though the inference error has already replied.
    app.tick();
    assert!(waiting.as_mut().now_or_never().is_none());
    assert!(!app
        .world()
        .get_component::<ActiveInference>(agent)
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .is_finished());
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !app
            .world()
            .get_component::<ActiveInference>(agent)
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .is_finished()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    app.tick();
    tokio::time::timeout(Duration::from_secs(1), &mut waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(
        app.world()
            .get_component::<ActiveInference>(agent)
            .unwrap()
            .current
            .is_some(),
        "wake must retain the handle for finish_turn"
    );

    // Stale request and VM notifications must not wake the current Driver.
    app.tick();
    let mut stale_request = cleanup.clone();
    stale_request.request_id = "old-request".into();
    let mut stale_instance = cleanup.clone();
    stale_instance.instance.start_id = "old-instance".into();
    app.world().send_event(stale_request);
    app.world().send_event(stale_instance);
    app.tick();
    let mut next_wait = Box::pin(DriverWakeSlot::wait(wake));
    assert!(next_wait.as_mut().now_or_never().is_none());
    app.world().send_event(cleanup);
    app.tick();
    tokio::time::timeout(Duration::from_secs(1), &mut next_wait)
        .await
        .unwrap()
        .unwrap();
    drop(sender);
    server.await.unwrap();
}

#[tokio::test]
async fn cleanup_event_retries_next_frame_until_actual_handle_finishes() {
    use app_runtime_plugin::RuntimePlugin;
    use core_plugin::App;
    use lua_runtime_core::{DriverLuaVmInstance, DriverWakeSlot};

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    app.add_system(RuntimePlugin::UPDATE, crate::system::inference_system);
    let agent = app.world_mut().spawn();
    let instance = DriverLuaVmInstance {
        owner: agent,
        start_id: "current-instance".into(),
    };
    let wake = DriverWakeSlot::shared();
    app.world_mut().insert_component(
        agent,
        DriverLuaVm {
            instance: instance.clone(),
            wake: wake.clone(),
            ready: Arc::new(Mutex::new(None)),
            ended: None,
            cancel: CancellationToken::new(),
        },
    );
    let (release, joined) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = joined.await;
    });
    app.world_mut().insert_component(
        agent,
        ActiveInference {
            current: Some(attach_handle(
                InferenceHandle {
                    request_id: "request".into(),
                    turn: "turn".into(),
                    cancel: CancellationToken::new(),
                    stop_stream: Arc::new(|| {}),
                    task: Arc::new(inference_core::InferenceTaskState::default()),
                },
                task,
            )),
        },
    );
    app.world().send_event(InferenceCleanupFinished {
        agent,
        request_id: "request".into(),
        instance,
    });
    let mut waiting = Box::pin(DriverWakeSlot::wait(wake));
    assert!(waiting.as_mut().now_or_never().is_none());
    app.tick();
    assert!(waiting.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !app
            .world()
            .get_component::<ActiveInference>(agent)
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .is_finished()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    app.fast_forward_tick();
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap();
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

fn status_error(body: &[u8], headers: &[(&str, &str)]) -> InferenceError {
    let mut response_headers = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        response_headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    format_provider_status_error(
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        &response_headers,
        body,
        None,
        &[],
    )
}

#[test]
fn status_error_json_allowlists_fields_even_when_message_contains_request_id() {
    let failed = status_error(br#"{"error":{"type":"service_unavailable_error","code":"overloaded","request_id":"nested-id","message":"Service is too busy (request_id: embedded-id)","api_key":"unknown-secret","url":"https://private.invalid"},"request_id":"root-id","unknown":"ignored"}"#, &[("x-request-id", "header-id")]);
    assert_eq!(failed.kind, Kind::ResponseStatus);
    assert_eq!(failed.message, "HTTP 503 Service Unavailable; type=service_unavailable_error; code=overloaded; request_id=nested-id; message=Service is too busy (request_id: embedded-id)");
}

#[test]
fn status_error_captured_yakumo_503_preserves_busy_detail_and_embedded_request_id() {
    let captured = br#"{"error":{"message":"Service is too busy. We advise users to temporarily switch to alternative LLM API service providers. (request_id: 57b431fb-e5fd-43b9-94ce-785f607bcfd7)","type":"service_unavailable_error","param":null,"code":"service_unavailable_error"}}"#;
    let failed = status_error(captured, &[]);
    assert_eq!(failed.message, "HTTP 503 Service Unavailable; type=service_unavailable_error; code=service_unavailable_error; request_id=57b431fb-e5fd-43b9-94ce-785f607bcfd7; message=Service is too busy. We advise users to temporarily switch to alternative LLM API service providers. (request_id: 57b431fb-e5fd-43b9-94ce-785f607bcfd7)");
    println!("{}", failed);
}

#[test]
fn status_error_supports_string_anthropic_and_request_id_fallbacks() {
    assert_eq!(
        status_error(br#"{"error":"busy","request_id":"root-id"}"#, &[]).message,
        "HTTP 503 Service Unavailable; request_id=root-id; message=busy"
    );
    assert_eq!(status_error(br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &[("x-request-id", "anthropic-id")]).message, "HTTP 503 Service Unavailable; type=overloaded_error; request_id=anthropic-id; message=Overloaded");
    assert!(status_error(b"", &[("request-id", "fallback-id")])
        .message
        .contains("request_id=fallback-id"));
    assert!(status_error(
        br#"{"error":{"message":"busy"}}"#,
        &[("x-request-id", "preferred"), ("request-id", "other")]
    )
    .message
    .contains("request_id=preferred"));
}

#[test]
fn status_error_redacts_actual_credentials_before_truncating_fields() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer actual-private-bearer".parse().unwrap(),
    );
    headers.insert("x-api-key", "actual-private-api-key".parse().unwrap());
    let secrets = provider_error_secrets(&headers);
    let body = json!({"error":{"type":"actual-private-api-key","code":"Bearer actual-private-bearer","request_id":"id-actual-private-bearer","message":"Echo actual-private-api-key and actual-private-bearer and Bearer actual-private-bearer https://user:password@host.invalid/key?api_key=other-secret","unknown":"unknown-secret"}});
    let failed = format_provider_status_error(
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        &headers,
        body.to_string().as_bytes(),
        None,
        &secrets,
    );
    for forbidden in [
        "actual-private",
        "user:password",
        "host.invalid",
        "other-secret",
        "unknown-secret",
    ] {
        assert!(!failed.message.contains(forbidden), "{}", failed.message);
    }
    assert!(failed.message.contains("[REDACTED]"));
    assert!(failed.message.contains("[URL REDACTED]"));
    let boundary = format!("{}actual-private-api-key", "界".repeat(500));
    assert!(!safe_error_field(&boundary, &secrets, 510).contains("actual-private"));
}

#[test]
fn status_error_bounds_unicode_and_handles_empty_malformed_html_and_plain_text() {
    for body in [b"".as_slice(), b"{broken json", b"[]"] {
        let failed = status_error(body, &[]);
        assert!(failed.message.starts_with("HTTP 503 Service Unavailable;"));
        assert!(!failed.message.contains("broken json"));
    }
    assert_eq!(
        status_error(
            b"<html>private diagnostic</html>",
            &[("content-type", "text/html; charset=utf-8")]
        )
        .message,
        "HTTP 503 Service Unavailable; message=non-JSON error response; content_type=text/html"
    );
    assert_eq!(
        status_error(b"  busy\n retry later  ", &[("content-type", "text/plain")]).message,
        "HTTP 503 Service Unavailable; message=busy retry later"
    );
    let body = json!({"error":{"type":"t".repeat(300),"code":"c".repeat(300),"message":"界".repeat(10000)},"request_id":"important-id"}).to_string();
    let failed = status_error(body.as_bytes(), &[]);
    assert!(failed.message.len() <= MAX_STATUS_ERROR_BYTES);
    assert!(failed.message.contains("request_id=important-id"));
    assert!(failed.message.contains("message=界"));
}

async fn error_http_server(
    response: String,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (release, wait) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        let mut byte = [0];
        while !header.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
        }
        let length = String::from_utf8_lossy(&header)
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")?
                    .trim()
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(0);
        socket.read_exact(&mut vec![0; length]).await.unwrap();
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = wait.await;
    });
    (url, release, task)
}

fn outbound_to(url: String) -> ProviderHttpRequest {
    build_request(
        DomainRequest {
            messages: request().messages,
            tools: vec![],
            config: request().config,
        },
        &route(url),
    )
    .unwrap()
}

#[tokio::test]
async fn status_error_real_json_redacts_the_actual_outbound_key() {
    let body = r#"{"error":{"type":"service_unavailable_error","code":"busy","message":"Service is too busy; echoed test"}}"#;
    let (url, release, server) = error_http_server(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nX-Request-Id: real-header-id\r\n\r\n{body}", body.len())).await;
    let (sender, _receiver) = mpsc::channel(2);
    let failed = read_provider_with_timeouts(
        reqwest::Client::new(),
        outbound_to(url),
        CancellationToken::new(),
        sender,
        (Duration::from_secs(1), Duration::from_secs(1)),
    )
    .await
    .unwrap_err();
    assert_eq!(failed.message, "HTTP 503 Service Unavailable; type=service_unavailable_error; code=busy; request_id=real-header-id; message=Service is too busy; echoed [REDACTED]");
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn status_error_oversize_prefix_stops_without_waiting_for_stream_end() {
    let prefix = "x".repeat(MAX_ERROR_BODY_BYTES);
    let (url, release, server) = error_http_server(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{prefix}", MAX_ERROR_BODY_BYTES * 2)).await;
    let (sender, _receiver) = mpsc::channel(2);
    let failed = tokio::time::timeout(
        Duration::from_secs(1),
        read_provider_with_timeouts(
            reqwest::Client::new(),
            outbound_to(url),
            CancellationToken::new(),
            sender,
            (Duration::from_secs(1), Duration::from_secs(10)),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(failed.kind, Kind::ResponseStatus);
    assert!(failed.message.starts_with("HTTP 503 Service Unavailable"));
    assert!(failed.message.ends_with("error body truncated"));
    assert!(failed.message.len() <= MAX_STATUS_ERROR_BYTES);
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn status_error_body_timeout_and_disconnect_preserve_http_status() {
    for disconnect in [false, true] {
        let (url, release, server) = error_http_server(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\n\r\npartial".into(),
        )
        .await;
        let mut release = Some(release);
        if disconnect {
            let _ = release.take().unwrap().send(());
        }
        let (sender, _receiver) = mpsc::channel(2);
        let failed = read_provider_with_timeouts(
            reqwest::Client::new(),
            outbound_to(url),
            CancellationToken::new(),
            sender,
            (Duration::from_secs(1), Duration::from_millis(40)),
        )
        .await
        .unwrap_err();
        assert_eq!(failed.kind, Kind::ResponseStatus);
        assert!(failed.message.starts_with("HTTP 503 Service Unavailable"));
        assert!(failed.message.ends_with(if disconnect {
            "error body disconnected"
        } else {
            "error body read timed out"
        }));
        if let Some(release) = release {
            let _ = release.send(());
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn status_error_trickling_body_does_not_reset_read_deadline() {
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
        socket
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 1000\r\n\r\n")
            .await
            .unwrap();
        for _ in 0..100 {
            if socket.write_all(b"x").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    let failed = tokio::time::timeout(
        Duration::from_millis(300),
        read_provider_status_error(
            response,
            &CancellationToken::new(),
            Duration::from_millis(60),
            &[],
        ),
    )
    .await
    .unwrap();
    assert_eq!(failed.kind, Kind::ResponseStatus);
    assert!(failed.message.ends_with("error body read timed out"));
    server.await.unwrap();
}

#[tokio::test]
async fn status_error_body_cancellation_remains_immediate() {
    let (url, release, server) = error_http_server(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\n\r\npartial".into(),
    )
    .await;
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move {
        read_provider_status_error(response, &worker_cancel, Duration::from_secs(60), &[]).await
    });
    tokio::task::yield_now().await;
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .kind,
        Kind::Cancelled
    );
    let _ = release.send(());
    server.await.unwrap();
}

fn attach_handle(handle: InferenceHandle, task: tokio::task::JoinHandle<()>) -> InferenceHandle {
    handle.attach_task(task).unwrap();
    handle
}

fn pending_handle() -> InferenceHandle {
    InferenceHandle {
        request_id: "request".into(),
        turn: "turn".into(),
        cancel: CancellationToken::new(),
        stop_stream: Arc::new(|| {}),
        task: Arc::new(inference_core::InferenceTaskState::default()),
    }
}

#[tokio::test]
async fn shared_handle_waiters_cover_pending_launch_and_real_join() {
    let handle = pending_handle();
    let clone = handle.clone();
    let first = handle.wait_finished();
    let second = clone.wait_finished();
    tokio::pin!(first, second);
    assert!(!handle.is_finished());
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    let (release, blocked) = oneshot::channel();
    handle
        .attach_task(tokio::spawn(async move {
            blocked.await.unwrap();
        }))
        .unwrap();
    assert!(!clone.is_finished());
    // Both futures observed the launch transition before the actual join.
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        let (first, second) = tokio::join!(first, second);
        first.unwrap();
        second.unwrap();
    })
    .await
    .unwrap();
    assert!(handle.is_finished());
    assert!(clone.is_finished());
    handle.wait_finished().await.unwrap();
}

#[tokio::test]
async fn launch_guard_preserves_original_reply_and_finishes_pending_handle() {
    let handle = pending_handle();
    let (reply, received) = oneshot::channel();
    let guard = InferenceLaunchGuard {
        fallback: InferenceLaunchFallback {
            handle: handle.clone(),
            slot: Arc::new(Mutex::new(Some(InferenceContext {
                request: request(),
                reply: Some(reply),
                route: route("http://127.0.0.1:1".into()),
                tools: vec![],
                client: reqwest::Client::new(),
                gate: Arc::new(Mutex::new(StreamGate::default())),
                delivery: delivery(),
                cleanup_notification: None,
                cleanup_gate: None,
            }))),
        },
        armed: true,
    };
    assert!(!handle.is_finished());
    drop(guard);
    assert_eq!(
        received.await.unwrap().unwrap_err().kind,
        Kind::TaskPanicked
    );
    assert!(handle.is_finished());
    assert_eq!(
        handle.wait_finished().await.unwrap_err().kind,
        Kind::TaskPanicked
    );
}

#[tokio::test]
async fn ordinary_launch_hands_supervisor_to_registered_shared_handle() {
    let handle = pending_handle();
    let (reply, received) = oneshot::channel();
    let input = request();
    input.cancel.cancel();
    let guard = InferenceLaunchGuard {
        fallback: InferenceLaunchFallback {
            handle: handle.clone(),
            slot: Arc::new(Mutex::new(Some(InferenceContext {
                request: input,
                reply: Some(reply),
                route: route("http://127.0.0.1:1".into()),
                tools: vec![],
                client: reqwest::Client::new(),
                gate: Arc::new(Mutex::new(StreamGate::default())),
                delivery: delivery(),
                cleanup_notification: None,
                cleanup_gate: None,
            }))),
        },
        armed: true,
    };
    assert!(launch_inference(guard).await.is_ok());
    handle.wait_finished().await.unwrap();
    assert_eq!(
        received.await.unwrap().unwrap().stop_reason,
        InferenceStopReason::Cancelled
    );
}

#[tokio::test]
async fn dropping_app_with_queued_launch_releases_context_and_original_reply() {
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::{AsyncContext, AsyncRuntimePlugin};
    use closure_plugin::{AppClosureExt, ClosurePlugin};
    use core_plugin::App;
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE);
    let handle = pending_handle();
    let (reply, received) = oneshot::channel();
    let launch = InferenceLaunchGuard {
        fallback: InferenceLaunchFallback {
            handle: handle.clone(),
            slot: Arc::new(Mutex::new(Some(InferenceContext {
                request: request(),
                reply: Some(reply),
                route: route("http://127.0.0.1:1".into()),
                tools: vec![],
                client: reqwest::Client::new(),
                gate: Arc::new(Mutex::new(StreamGate::default())),
                delivery: delivery(),
                cleanup_notification: None,
                cleanup_gate: None,
            }))),
        },
        armed: true,
    };
    let context = Arc::downgrade(&launch.fallback.slot);
    app.world()
        .send_async_closure(RuntimePlugin::UPDATE, move |_: AsyncContext| {
            launch_inference(launch)
        });
    assert!(!handle.is_finished());
    drop(app);
    assert!(
        context.upgrade().is_none(),
        "queued context must not keep its World event queue alive"
    );
    assert_eq!(
        received.await.unwrap().unwrap_err().kind,
        Kind::TaskPanicked
    );
    assert_eq!(
        handle.wait_finished().await.unwrap_err().kind,
        Kind::TaskPanicked
    );
}

#[tokio::test]
async fn dropping_last_polled_waiter_does_not_strand_another_shared_waiter() {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    struct CountWake(AtomicUsize);
    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let handle = pending_handle();
    let clone = handle.clone();
    let (release, blocked) = oneshot::channel();
    handle
        .attach_task(tokio::spawn(async move {
            blocked.await.unwrap();
        }))
        .unwrap();
    let first_wakes = Arc::new(CountWake(AtomicUsize::new(0)));
    let second_wakes = Arc::new(CountWake(AtomicUsize::new(0)));
    let first_waker = Waker::from(first_wakes.clone());
    let second_waker = Waker::from(second_wakes);
    let mut first = Box::pin(handle.wait_finished());
    let mut second = Box::pin(clone.wait_finished());
    assert!(matches!(
        first.as_mut().poll(&mut Context::from_waker(&first_waker)),
        Poll::Pending
    ));
    assert!(matches!(
        second
            .as_mut()
            .poll(&mut Context::from_waker(&second_waker)),
        Poll::Pending
    ));
    drop(second);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        first_wakes.0.load(Ordering::SeqCst) > 0,
        "supervisor completion must wake the surviving waiter"
    );
    first.await.unwrap();
}
