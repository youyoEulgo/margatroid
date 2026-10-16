use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use inference_plugin::{
    InferenceErrorKind, InferencePlugin, InferenceRequest, ReloadModelRoutes,
    ReloadModelRoutesResult,
};
use margatroid_types::agent_components::{
    Agent, AgentInfo, AgentState, ImportedValue, WorldAgentExt,
};
use margatroid_types::block_storage::AgentDatabaseHandle;
use margatroid_types::execution_components::AgentExecutionScope;
use margatroid_types::message_types::{AgentMessage, InferenceResponse, StopReason, UserImage};
use margatroid_types::model_components::ModelRuntimeCatalog;
use margatroid_types::resource_components::{
    AgentDriverModelConfig, CallableDefinitionSnapshot, CallableResource, ToolResource,
};
use memory_plugin::SqliteAgentDatabase;
use resource_id_plugin::{ResourceID, ResourceSet, WorldResourceExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn id(kind: &str) -> ResourceID {
    ResourceID::parse(&format!("{kind}:test/main")).unwrap()
}

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("models.toml");
        std::fs::write(&path, config).unwrap();
        let database = AgentDatabaseHandle::new(
            SqliteAgentDatabase::open(root.path().join("agent.sqlite")).unwrap(),
        );
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(resource_id_plugin::ResourcePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::PRE_UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(InferencePlugin::default().with_config_path(path));
        app.world_mut().register_library::<Agent>("agent").unwrap();
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            Agent {
                resourceid: id("agent"),
                workspace: id("workspace"),
                sandbox: None,
                info: AgentInfo {
                    model: AgentDriverModelConfig {
                        model: "test".into(),
                        temperature: None,
                        max_output_tokens: Some(128),
                        top_p: None,
                        stop: vec![],
                    },
                    project_root: root.path().into(),
                    home_root: root.path().into(),
                    driver: id("agent_driver"),
                    driver_dependencies: Arc::from([]),
                },
                lua: Default::default(),
                state: AgentState::Working,
                history: database.history(),
                database,
                import_bindings: Default::default(),
                mcl: Default::default(),
                execution: AgentExecutionScope::new(),
                turn: Some("http-turn".into()),
            },
        );
        Self { app, agent, root }
    }

    fn tool(&mut self) -> CallableResource {
        let resource = ToolResource {
            alias: "echo".into(),
            description: "Echo input".into(),
            input_schema: json!({"type":"object","properties":{"x":{"type":"integer"}}}),
            script_path: self.root.path().join("unused.lua"),
        };
        let callable = CallableResource {
            resource_id: id("tool"),
            alias: "echo".into(),
            definition: Arc::new(CallableDefinitionSnapshot::Tool(resource.clone())),
        };
        let mut set = ResourceSet::new();
        set.insert(id("tool"), resource);
        self.app.world_mut().insert_component(self.agent, set);
        self.app
            .world_mut()
            .get_component_mut::<Agent>(self.agent)
            .unwrap()
            .info
            .driver_dependencies = Arc::from([id("tool")]);
        self.app
            .world_mut()
            .agent_bind_import(
                &id("agent"),
                "echo".into(),
                ImportedValue::Tool(callable.clone()),
            )
            .unwrap();
        callable
    }

    fn infer(
        &mut self,
        messages: Vec<AgentMessage>,
        tools: Vec<CallableResource>,
    ) -> InferenceResponse {
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        self.app.world().send_event(InferenceRequest {
            id: "http-request".into(),
            agent: self.agent,
            messages,
            tools,
            turn: Some("http-turn".into()),
            cancel: tokio_util::sync::CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.tick();
            match rx.try_recv() {
                Ok(result) => {
                    assert_eq!(
                        self.app
                            .world()
                            .get_component::<Agent>(self.agent)
                            .unwrap()
                            .execution
                            .activity_count(),
                        0
                    );
                    return result.unwrap();
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(error) => panic!("inference reply lost: {error}"),
            }
            assert!(Instant::now() < deadline, "inference timed out");
            std::thread::yield_now();
        }
    }
}

struct Captured {
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

fn mock(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<Captured>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut captured = vec![];
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "mock received no request");
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("mock accept failed: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = vec![];
            let (header_end, headers, path) = loop {
                let mut buffer = [0; 2048];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let text = std::str::from_utf8(&bytes[..end]).unwrap();
                    let mut lines = text.split("\r\n");
                    let first = lines.next().unwrap();
                    assert!(first.starts_with("POST "));
                    let path = first.split_whitespace().nth(1).unwrap().to_owned();
                    let headers: HashMap<_, _> = lines
                        .map(|line| {
                            let (key, value) = line.split_once(':').unwrap();
                            (key.to_ascii_lowercase(), value.trim().to_owned())
                        })
                        .collect();
                    break (end + 4, headers, path);
                }
                assert!(bytes.len() < 65536);
            };
            let length: usize = headers["content-length"].parse().unwrap();
            while bytes.len() < header_end + length {
                let mut buffer = [0; 2048];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            captured.push(Captured {
                path,
                headers,
                body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
            });
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
            for chunk in response.as_bytes().chunks(11) {
                socket.write_all(chunk).unwrap();
            }
        }
        captured
    });
    (url, server)
}

fn route(url: &str, vendor: &str, format: &str) -> String {
    format!("[[models]]\nid='test'\nmodel='wire-model'\nbase_url='{url}'\napi_key='local-test-key'\nprovider='{vendor}'\napi_type='{format}'\n")
}

fn sse(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\r\ndata: {event}\r\n\r\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn messages_stream() -> String {
    sse(vec![
        json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":15,"cache_read_input_tokens":3}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
        json!({"type":"message_stop"}),
    ])
}

fn responses_stream(tool: bool) -> String {
    let mut events = vec![
        json!({"type":"response.created","response":{"status":"in_progress"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),
        json!({"type":"response.output_text.delta","output_index":0,"delta":"answer"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}}),
    ];
    events.extend([
        json!({"type":"response.output_item.added","output_index":2,"item":{"type":"reasoning","id":"rs_1","summary":[]}}),
        json!({"type":"response.reasoning_summary_text.delta","output_index":2,"delta":"think"}),
        json!({"type":"response.output_item.done","output_index":2,"item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"think"}],"encrypted_content":"local-opaque-state"}}),
    ]);
    if tool {
        events.extend([
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"echo","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"x\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"1}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"echo","arguments":"{\"x\":1}"}}),
        ]);
    }
    events.push(json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":11,"output_tokens":5,"input_tokens_details":{"cached_tokens":7}}}}));
    sse(events)
}

fn input() -> Vec<AgentMessage> {
    vec![
        AgentMessage::System {
            content: "system".into(),
        },
        AgentMessage::User {
            content: "hello".into(),
            images: vec![],
        },
    ]
}

fn images() -> Vec<UserImage> {
    vec![
        UserImage {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        },
        UserImage {
            media_type: "image/gif".into(),
            data: "R0lGODlh".into(),
        },
        UserImage {
            media_type: "image/jpeg".into(),
            data: "/9j/".into(),
        },
        UserImage {
            media_type: "image/webp".into(),
            data: "UklGRgAAAABXRUJQ".into(),
        },
    ]
}

fn assert_image_parts(value: &Value, format: &str, text: bool) {
    let parts = value.as_array().unwrap();
    assert_eq!(parts.len(), images().len() + usize::from(text));
    let offset = usize::from(text);
    if text {
        assert_eq!(parts[0]["text"], "hello");
        assert_eq!(
            parts[0]["type"],
            if format == "responses" {
                "input_text"
            } else {
                "text"
            }
        );
    }
    for (part, image) in parts[offset..].iter().zip(images()) {
        let url = format!("data:{};base64,{}", image.media_type, image.data);
        match format {
            "messages" => assert_eq!(
                part,
                &json!({"type":"image","source":{"type":"base64","media_type":image.media_type,"data":image.data}})
            ),
            "responses" => assert_eq!(part, &json!({"type":"input_image","image_url":url})),
            _ => assert_eq!(part, &json!({"type":"image_url","image_url":{"url":url}})),
        }
    }
}

#[test]
fn invalid_images_rejected_before_http_without_data_leaks() {
    for (vendor, format) in [
        ("openai", "chat_completions"),
        ("openai", "responses"),
        ("anthropic", "messages"),
        ("deepseek", "chat_completions"),
        ("deepseek", "messages"),
        ("deepseek", "responses"),
        ("openrouter", "chat_completions"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let mut fixture = Fixture::new(&route(&url, vendor, format));
        for image in [
            UserImage {
                media_type: "image/png".into(),
                data: "PRIVATE-INVALID-BASE64".into(),
            },
            UserImage {
                media_type: "image/jpeg".into(),
                data: "iVBORw0KGgo=".into(),
            },
        ] {
            let data = image.data.clone();
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            fixture.app.world().send_event(InferenceRequest {
                id: "invalid-image".into(),
                agent: fixture.agent,
                messages: vec![AgentMessage::Inject {
                    messages: vec![AgentMessage::User {
                        content: String::new(),
                        images: vec![image],
                    }],
                }],
                tools: vec![],
                turn: Some("http-turn".into()),
                cancel: tokio_util::sync::CancellationToken::new(),
                reply: Arc::new(Mutex::new(Some(tx))),
            });
            fixture.app.tick();
            let error = rx.try_recv().unwrap().unwrap_err();
            assert_eq!(error.kind, InferenceErrorKind::InvalidMessages);
            assert!(!error.to_string().contains(&data));
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
            assert_eq!(
                fixture
                    .app
                    .world()
                    .get_component::<Agent>(fixture.agent)
                    .unwrap()
                    .execution
                    .activity_count(),
                0
            );
        }
    }
}

fn assert_assistant(response: &InferenceResponse, expected: StopReason) {
    assert_eq!(response.stop_reason, expected);
    let AgentMessage::Assistant {
        content,
        stop_reason,
        ..
    } = &response.message
    else {
        panic!("not an assistant")
    };
    assert_eq!(content.as_deref(), Some("answer"));
    assert_eq!(stop_reason.as_ref(), Some(&response.stop_reason));
}

#[test]
fn every_supported_vendor_format_dispatches_real_http() {
    for (vendor, format, suffix) in [
        ("openai", "chat_completions", "chat/completions"),
        ("openai", "responses", "responses"),
        ("anthropic", "messages", "messages"),
        ("deepseek", "chat_completions", "chat/completions"),
        ("deepseek", "messages", "messages"),
        ("deepseek", "responses", "responses"),
        ("openrouter", "chat_completions", "chat/completions"),
    ] {
        let stream = match format {
            "messages" => messages_stream(),
            "responses" => responses_stream(false),
            _ => "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into(),
        };
        let (url, server) = mock(vec![stream.clone(), stream]);
        let mut fixture = Fixture::new(&route(&url, vendor, format));
        let mut history = input();
        history[1] = AgentMessage::User {
            content: "hello".into(),
            images: images(),
        };
        let first = fixture.infer(history.clone(), vec![]);
        assert_assistant(&first, StopReason::Completed);
        history.push(first.message);
        history.push(AgentMessage::User {
            content: String::new(),
            images: images(),
        });
        assert_assistant(&fixture.infer(history, vec![]), StopReason::Completed);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            let (key, index) = if format == "responses" {
                ("input", 1)
            } else if format == "messages" {
                ("messages", 0)
            } else {
                ("messages", 1)
            };
            assert_image_parts(&request.body[key][index]["content"], format, true);
        }
        let key = if format == "responses" {
            "input"
        } else {
            "messages"
        };
        assert_image_parts(
            &requests[1].body[key].as_array().unwrap().last().unwrap()["content"],
            format,
            false,
        );
        let request = &requests[0];
        assert_eq!(request.path, format!("/v1/{suffix}"), "{vendor}/{format}");
        assert_eq!(request.headers["content-type"], "application/json");
        assert_eq!(request.headers["accept"], "text/event-stream");
        assert_eq!(request.body["model"], "wire-model");
        assert_eq!(request.body["stream"], true);
        if format == "messages" {
            assert_eq!(request.headers["x-api-key"], "local-test-key");
            assert_eq!(request.headers["anthropic-version"], "2023-06-01");
            assert!(!request.headers.contains_key("authorization"));
            assert_eq!(request.body["max_tokens"], 128);
            assert_eq!(
                request.body["system"],
                json!([{"type":"text","text":"system"}])
            );
            assert_eq!(request.body["messages"][0]["role"], "user");
        } else {
            assert_eq!(request.headers["authorization"], "Bearer local-test-key");
            assert!(!request.headers.contains_key("x-api-key"));
            if format == "responses" {
                assert_eq!(request.body["store"], false);
                assert_eq!(request.body["max_output_tokens"], 128);
                assert_eq!(
                    request.body["include"],
                    json!(["reasoning.encrypted_content"])
                );
                assert_eq!(request.body["input"][0]["role"], "system");
            } else {
                assert_eq!(request.body["max_tokens"], 128);
                assert_eq!(request.body["messages"][0]["content"], "system");
            }
        }
    }
}

#[test]
fn anthropic_signed_messages_replay_through_public_inference() {
    let (url, server) = mock(vec![messages_stream(), messages_stream()]);
    let mut fixture = Fixture::new(&route(&url, "anthropic", "messages"));
    let first = fixture.infer(input(), vec![]);
    assert_assistant(&first, StopReason::Completed);
    let AgentMessage::Assistant {
        reasoning,
        replay,
        usage,
        ..
    } = &first.message
    else {
        unreachable!()
    };
    assert_eq!(reasoning.as_deref(), Some("think"));
    assert_eq!(replay.as_ref().unwrap()["api_format"], "messages");
    assert_eq!(usage.as_ref().unwrap().cache_hit_tokens, 3);
    let mut history = input();
    history.push(first.message);
    history.push(AgentMessage::User {
        content: "continue".into(),
        images: vec![],
    });
    assert_assistant(&fixture.infer(history, vec![]), StopReason::Completed);
    let requests = server.join().unwrap();
    assert_eq!(
        requests[1].body["messages"][1]["content"],
        json!([
            {"type":"thinking","thinking":"think","signature":"signed"},
            {"type":"text","text":"answer"},
        ])
    );
}

#[test]
fn openai_responses_reassembles_function_arguments_and_replays_items() {
    let (url, server) = mock(vec![responses_stream(true), responses_stream(false)]);
    let mut fixture = Fixture::new(&route(&url, "openai", "responses"));
    let tool = fixture.tool();
    let first = fixture.infer(input(), vec![tool.clone()]);
    assert_assistant(&first, StopReason::ToolCalls);
    let AgentMessage::Assistant {
        tool_calls,
        replay,
        usage,
        ..
    } = &first.message
    else {
        unreachable!()
    };
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].tool_name, "echo");
    assert_eq!(tool_calls[0].arguments, "{\"x\":1}");
    assert_eq!(replay.as_ref().unwrap()["api_format"], "responses");
    assert_eq!(usage.as_ref().unwrap().cache_hit_tokens, 7);
    let mut history = input();
    history.push(first.message);
    history.push(AgentMessage::ToolCall {
        tool_call_id: "call_1".into(),
        content: "ok".into(),
        resource_id: Some(id("tool")),
        tool_name: "echo".into(),
        images: vec![],
        failed: false,
    });
    assert_assistant(&fixture.infer(history, vec![tool]), StopReason::Completed);
    let requests = server.join().unwrap();
    assert_eq!(requests[0].body["tools"][0]["type"], "function");
    let replayed = requests[1].body["input"].as_array().unwrap();
    assert!(replayed.iter().any(
        |item| item["type"] == "reasoning" && item["encrypted_content"] == "local-opaque-state"
    ));
    assert!(replayed
        .iter()
        .any(|item| item["type"] == "function_call" && item["arguments"] == "{\"x\":1}"));
    assert!(replayed
        .iter()
        .any(|item| item["type"] == "function_call_output"
            && item["call_id"] == "call_1"
            && item["output"] == "ok"));
}

#[test]
fn invalid_format_routes_reload_atomically_without_replacing_dispatch() {
    let (url, server) = mock(vec![messages_stream()]);
    let valid = route(&url, "anthropic", "messages");
    let mut fixture = Fixture::new(&valid);
    let original = fixture
        .app
        .world()
        .get_resource::<ModelRuntimeCatalog>()
        .unwrap()
        .clone();
    for (index, invalid, expected) in [
        (
            0,
            route(&url, "anthropic", "responses"),
            InferenceErrorKind::InvalidModelRoute,
        ),
        (
            1,
            route(&url, "openai", "messages"),
            InferenceErrorKind::InvalidModelRoute,
        ),
        (
            2,
            route(&url, "openrouter", "responses"),
            InferenceErrorKind::InvalidModelRoute,
        ),
        (
            3,
            valid.replace("provider='anthropic'\n", ""),
            InferenceErrorKind::ConfigDecodeFailed,
        ),
        (
            4,
            valid.replace("api_type='messages'", "api_type='anthropic'"),
            InferenceErrorKind::ConfigDecodeFailed,
        ),
    ] {
        let candidate = format!(
            "{}\n{}",
            valid.replace("id='test'", "id='would-be-added'"),
            invalid
        );
        std::fs::write(fixture.root.path().join("models.toml"), candidate).unwrap();
        let reload_id = format!("bad-{index}");
        fixture.app.world().send_event(ReloadModelRoutes {
            id: reload_id.clone(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            fixture.app.tick();
            if let Some(result) = fixture
                .app
                .world()
                .event_reader::<ReloadModelRoutesResult>()
                .into_iter()
                .find(|event| event.id == reload_id)
            {
                assert_eq!(result.result.as_ref().unwrap_err().kind, expected);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            fixture
                .app
                .world()
                .get_resource::<ModelRuntimeCatalog>()
                .unwrap(),
            &original
        );
        assert_eq!(
            fixture
                .app
                .world()
                .get_resource::<inference_plugin::InferenceConfig>()
                .unwrap()
                .model_count(),
            1
        );
    }
    assert_assistant(&fixture.infer(input(), vec![]), StopReason::Completed);
    assert_eq!(server.join().unwrap()[0].path, "/v1/messages");
}

#[test]
fn tool_images_preserve_parallel_batches_native_outputs_and_repeat_requests() {
    for (vendor, format) in [
        ("openai", "chat_completions"),
        ("openai", "responses"),
        ("anthropic", "messages"),
        ("deepseek", "chat_completions"),
        ("deepseek", "messages"),
        ("deepseek", "responses"),
        ("openrouter", "chat_completions"),
    ] {
        let stream = match format {
            "messages" => messages_stream(),
            "responses" => responses_stream(false),
            _ => "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into(),
        };
        let (url, server) = mock(vec![stream.clone(), stream]);
        let mut fixture = Fixture::new(&route(&url, vendor, format));
        let assistant = |ids: &[&str]| AgentMessage::Assistant {
            reasoning: None,
            replay: match format {
                "messages" => Some(
                    json!({"api_format":"messages","items":[{"type":"thinking","thinking":"thought","signature":"signed"}]}),
                ),
                "responses" => Some(
                    json!({"api_format":"responses","items":[{"type":"reasoning","id":"reasoning-1","summary":[],"encrypted_content":"opaque"}]}),
                ),
                _ => None,
            },
            content: None,
            tool_calls: ids
                .iter()
                .map(|id| margatroid_types::message_types::ToolCall {
                    id: (*id).into(),
                    tool_name: "echo".into(),
                    arguments: "{}".into(),
                })
                .collect(),
            usage: None,
            stop_reason: Some(StopReason::ToolCalls),
        };
        let result =
            |id: &str, text: &str, pictures: Vec<UserImage>, failed| AgentMessage::ToolCall {
                resource_id: Some(self::id("tool")),
                tool_call_id: id.into(),
                tool_name: "echo".into(),
                content: text.into(),
                images: pictures,
                failed,
            };
        let mut history = input();
        history.extend([
            assistant(&["a", "b"]),
            result("a", "hello", images(), false),
            result("b", "plain", vec![], false),
            assistant(&["c"]),
            result("c", "", images(), true),
        ]);
        let original = serde_json::to_value(&history).unwrap();
        let tool = fixture.tool();
        for _ in 0..2 {
            assert_assistant(
                &fixture.infer(history.clone(), vec![tool.clone()]),
                StopReason::Completed,
            );
        }
        assert_eq!(serde_json::to_value(&history).unwrap(), original);
        let requests = server.join().unwrap();
        assert_eq!(requests[0].body, requests[1].body);
        let body = &requests[0].body;
        match format {
            "messages" => {
                let results = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|message| message["content"].as_array().unwrap())
                    .filter(|part| part["type"] == "tool_result")
                    .collect::<Vec<_>>();
                assert_eq!(results.len(), 3);
                assert_eq!(results[0]["tool_use_id"], "a");
                assert_image_parts(&results[0]["content"], format, true);
                assert_eq!(results[1]["tool_use_id"], "b");
                assert_eq!(results[1]["content"], "plain");
                assert_eq!(results[2]["tool_use_id"], "c");
                assert_image_parts(&results[2]["content"], format, false);
                assert_eq!(results[2]["is_error"], true);
            }
            "responses" => {
                let items = body["input"].as_array().unwrap();
                assert_eq!(
                    items.iter().filter(|item| item["role"] == "user").count(),
                    1
                );
                let outputs = items
                    .iter()
                    .filter(|item| item["type"] == "function_call_output")
                    .collect::<Vec<_>>();
                assert_eq!(outputs.len(), 3);
                assert_eq!(outputs[0]["call_id"], "a");
                assert_image_parts(&outputs[0]["output"], format, true);
                assert_eq!(outputs[1]["call_id"], "b");
                assert_eq!(outputs[1]["output"], "plain");
                assert_eq!(outputs[2]["call_id"], "c");
                assert_image_parts(&outputs[2]["output"], format, false);
                assert!(items
                    .iter()
                    .any(|item| item["encrypted_content"] == "opaque"));
            }
            _ => {
                let messages = body["messages"].as_array().unwrap();
                let mut pending = std::collections::HashSet::new();
                let mut synthetic = Vec::new();
                let mut linked = Vec::new();
                for message in messages {
                    if let Some(calls) = message["tool_calls"].as_array() {
                        assert!(pending.is_empty());
                        for call in calls {
                            pending.insert(call["id"].as_str().unwrap());
                        }
                    }
                    if message["role"] == "tool" {
                        let id = message["tool_call_id"].as_str().unwrap();
                        assert!(pending.remove(id));
                        linked.push(id);
                        assert!(message["content"].is_string());
                    } else if message["role"] == "user" {
                        assert!(pending.is_empty(), "user interrupted parallel results");
                        if message["content"].is_array() {
                            synthetic.push(message);
                        }
                    }
                }
                assert!(pending.is_empty());
                assert_eq!(linked, ["a", "b", "c"]);
                assert_eq!(synthetic.len(), 2);
                for (message, id) in synthetic.into_iter().zip(["a", "c"]) {
                    let blocks = message["content"].as_array().unwrap();
                    assert_eq!(blocks.len(), images().len() * 2);
                    for (pair, image) in blocks.chunks_exact(2).zip(images()) {
                        assert_eq!(pair[0]["text"], format!("tool_call_id: {id}"));
                        assert_eq!(
                            pair[1]["image_url"]["url"],
                            format!("data:{};base64,{}", image.media_type, image.data)
                        );
                    }
                }
            }
        }
    }
}
