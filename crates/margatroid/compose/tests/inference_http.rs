mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use address::Address;
use agent_core::AgentState;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use inference_core::{
    ActiveInference, InferenceError, InferenceErrorKind as Kind, InferenceImage, InferenceMessage,
    InferenceParameters, InferenceRequest, InferenceResponse, InferenceStopReason,
    InferenceToolCall, InitInference,
};
use inference_plugin::InferencePlugin;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_resource_core::Tools;

struct HttpServer {
    url: String,
    received: mpsc::Receiver<(String, String, Value)>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HttpServer {
    fn new(trace: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (sender, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let thread = std::thread::spawn(move || {
            while !shutdown.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(_) => return,
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") && header.len() < 65536 {
                    if stream.read_exact(&mut byte).is_err() {
                        return;
                    }
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let _ = sender.send((path, header, serde_json::from_slice(&body).unwrap()));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    trace.len(),
                    trace
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            received,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn trace(format: &str) -> String {
    let events = match format {
        "messages" => vec![
            json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
        "responses" => vec![
            json!({"type":"response.created"}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","content":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"ok"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"ok"}]}}),
            json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":2}}}),
        ],
        _ => {
            return format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2}})
            );
        }
    };
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
    frames: client_core::ClientReceiver,
}

impl Fixture {
    fn new(url: &str, provider: &str, api_type: &str) -> Self {
        Self::build(url, provider, api_type, false)
    }

    fn with_dto(url: &str) -> Self {
        Self::build(url, "openai", "chat_completions", true)
    }

    fn build(url: &str, provider: &str, api_type: &str, dto: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("models.toml");
        std::fs::write(&config, format!("[[models]]\nid='route'\nmodel='upstream-model'\nprovider='{provider}'\napi_type='{api_type}'\nbase_url='{url}'\napi_key='private-test-key'\ncontext_window='128k'\n")).unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(InferencePlugin::open(&config).unwrap());
        if dto {
            app.add_plugin(axum_plugin::AxumPlugin::bind(
                "127.0.0.1:0".parse().unwrap(),
            ));
            app.add_plugin(client_plugin::ClientPlugin::new());
            app.world_mut().insert_resource(
                config_core::MargatroidConfig::new(
                    "127.0.0.1:0".parse().unwrap(),
                    config_core::LogLevel::Info,
                    None,
                )
                .unwrap(),
            );
            app.add_plugin(dto_plugin::DtoPlugin::new());
        }
        let (frames_sender, frames) = client_core::ClientSender::channel(256);
        if !dto {
            let clients = client_core::Clients::default();
            let id = client_core::ConnectionId::new(1);
            clients.adopt(id, frames_sender).unwrap();
            let client = app.world_mut().spawn();
            let identity = std::sync::Arc::new(client_core::ClientIdentity {
                client,
                address: "client:webui/inference-fixture:1".into(),
                client_type: "webui".into(),
                name: "inference-fixture".into(),
            });
            clients.commit_registration(id, identity.clone()).unwrap();
            app.world_mut().insert_resource(clients);
            app.world_mut().insert_component(
                client,
                client_core::Client {
                    connection_id: id,
                    identity,
                },
            );
        }
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            address::Address::new("workspace:test/unit:dev").unwrap(),
        );
        app.world_mut().insert_component(
            workspace,
            Address::new("workspace:margatroid/demo:dev").unwrap(),
        );
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new(
                "demo".into(),
                root.path().join("workspace.yaml"),
                root.path().to_owned(),
                workspace_core::WorkspaceState::Running,
                std::collections::BTreeSet::new(),
                None,
                crate::test_workspace_context(),
            ),
        );
        let agent = app.world_mut().spawn();
        agent_core::AgentTaskBinding::reserve(
            app.world_mut(),
            agent,
            agent_core::AgentCreator {
                address: "workspace:margatroid/test:dev".into(),
                task_id: "fixture-driver".into(),
            },
        );
        let agent_updates = app
            .world()
            .get_resource::<agent_core::AgentUpdateContext>()
            .cloned()
            .unwrap_or_else(test_agent_context);
        app.world_mut().insert_component(
            agent,
            agent_core::Agent::new(
                "test".into(),
                workspace,
                AgentState::Running,
                Some("turn".into()),
                agent_updates.clone(),
            ),
        );
        app.world_mut().insert_component(
            agent,
            Address::new("agent:margatroid-test-dev/agent:dev").unwrap(),
        );
        install_vm(app.world_mut(), agent, "driver-fixture");
        let (sender, mut reply) = oneshot::channel();
        app.world().emit_event(InitInference {
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        app.tick();
        assert!(reply.try_recv().unwrap().is_ok());
        Self {
            app,
            agent,
            root,
            frames,
        }
    }

    fn request(
        &self,
        id: &str,
    ) -> (
        InferenceRequest,
        oneshot::Receiver<Result<InferenceResponse, InferenceError>>,
    ) {
        let (sender, receiver) = oneshot::channel();
        (
            InferenceRequest {
                id: id.into(),
                agent: self.agent,
                turn: Some("turn".into()),
                messages: vec![InferenceMessage::User {
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
                reply: Arc::new(Mutex::new(Some(sender))),
            },
            receiver,
        )
    }

    fn run(
        &mut self,
        request: InferenceRequest,
        reply: oneshot::Receiver<Result<InferenceResponse, InferenceError>>,
    ) -> Result<InferenceResponse, InferenceError> {
        self.app.world().emit_event(request);
        self.response(reply)
    }

    fn response(
        &mut self,
        mut reply: oneshot::Receiver<Result<InferenceResponse, InferenceError>>,
    ) -> Result<InferenceResponse, InferenceError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            match reply.try_recv() {
                Ok(result) => return result,
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(error) => panic!("inference reply closed: {error}"),
            }
            assert!(Instant::now() < deadline, "inference reply timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self
            .app
            .world()
            .get_component::<ActiveInference>(self.agent)
            .unwrap()
            .current
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn tool(&mut self, description: &str) {
        let directory = self.root.path().join("tool");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("tool.toml"),
            format!("schema_version=1\ndescription='{description}'\n"),
        )
        .unwrap();
        std::fs::write(directory.join("input.schema.json"), "{\"type\":\"object\"}").unwrap();
        std::fs::write(directory.join("main.lua"), "function configure(arguments, context) return {timeout_ms=30000} end\nfunction execute() end").unwrap();
        let mut tools = Tools::new(crate::test_probe());
        tools
            .insert("visible".into(), common::tool(directory.join("tool.toml")))
            .unwrap();
        self.app.world_mut().insert_component(self.agent, tools);
    }
}

#[test]
fn all_seven_provider_api_combinations_use_explicit_auth_endpoints_and_strict_streams() {
    for (provider, format) in [
        ("openai", "chat_completions"),
        ("openai", "responses"),
        ("anthropic", "messages"),
        ("deepseek", "chat_completions"),
        ("deepseek", "messages"),
        ("deepseek", "responses"),
        ("openrouter", "chat_completions"),
    ] {
        let server = HttpServer::new(trace(format));
        let mut fixture = Fixture::new(&server.url, provider, format);
        let (request, reply) = fixture.request("request");
        let response = fixture.run(request, reply).unwrap();
        assert_eq!(
            response.content.as_deref(),
            Some("ok"),
            "{provider}/{format}"
        );
        assert_eq!(response.stop_reason, InferenceStopReason::Completed);
        assert_eq!(response.usage.unwrap().output_tokens, 2);
        let mut frames = Vec::new();
        while let Ok(frame) = fixture.frames.try_recv() {
            let client_core::ClientFrame::Text(text) = frame else {
                panic!("inference must emit text frames")
            };
            frames.push(serde_json::from_str::<Value>(&text).unwrap());
        }
        assert!(
            frames.iter().any(
                |frame| frame["type"] == "agent.message.chunk.sync" && frame["content"] == "ok"
            )
        );
        assert_eq!(frames.last().unwrap()["type"], "agent.message.full.sync");
        assert!(frames.iter().all(|frame| frame["generation"] == "request"
            && frame.get("instance").is_none()
            && frame.get("agent_instance").is_none()));
        let (path, headers, body) = server
            .received
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            path,
            format!(
                "/v1/{}",
                if format == "chat_completions" {
                    "chat/completions"
                } else {
                    format
                }
            )
        );
        assert_eq!(body["model"], "upstream-model");
        assert_eq!(body["stream"], true);
        if format == "messages" {
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("x-api-key: private-test-key")
            );
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("anthropic-version: 2023-06-01")
            );
        } else {
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer private-test-key")
            );
        }
        if format == "responses" {
            assert_eq!(body["store"], false);
            assert!(body.get("previous_response_id").is_none());
        }
    }
}

#[test]
fn tool_entries_are_read_fresh_and_inference_uses_cached_definitions() {
    let server = HttpServer::new(trace("chat_completions"));
    let mut fixture = Fixture::new(&server.url, "openai", "chat_completions");
    for (id, description) in [
        ("first", "first-description"),
        ("second", "changed-description"),
    ] {
        fixture.tool(description);
        let (request, reply) = fixture.request(id);

        fixture.run(request, reply).unwrap();
        let (_, _, body) = server
            .received
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "visible");
        assert_eq!(body["tools"][0]["function"]["description"], description);
        fixture.idle();
    }
    let path = fixture.root.path().join("tool/tool.toml");
    assert_eq!(
        path.parent(),
        Some(fixture.root.path().join("tool").as_path())
    );
    std::fs::remove_file(path).unwrap();
    let (request, reply) = fixture.request("deleted");

    fixture.run(request, reply).unwrap();
    let (_, _, body) = server
        .received
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(body["tools"][0]["function"]["name"], "visible");
    assert_eq!(
        body["tools"][0]["function"]["description"],
        "changed-description"
    );
}

#[test]
fn every_inference_uses_all_current_tool_entries() {
    let server = HttpServer::new(trace("chat_completions"));
    let mut fixture = Fixture::new(&server.url, "openai", "chat_completions");
    let (request, reply) = fixture.request("registered-after-request");
    fixture.tool("first tool");
    let tools = fixture
        .app
        .world_mut()
        .get_component_mut::<Tools>(fixture.agent)
        .unwrap();
    let mut second = tools.get("visible").unwrap().clone();
    second.description = "second tool".into();
    second.owner_task_id = Some("dynamic-owner".into());
    tools.insert("second".into(), second.clone()).unwrap();
    assert_eq!(
        tools.insert("second".into(), second).unwrap_err().kind,
        tool_resource_core::ToolResourceErrorKind::AliasConflict
    );
    fixture.run(request, reply).unwrap();
    let (_, _, body) = server
        .received
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let definitions = body["tools"].as_array().unwrap();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0]["function"]["name"], "second");
    assert_eq!(definitions[0]["function"]["description"], "second tool");
    assert_eq!(definitions[1]["function"]["name"], "visible");
    fixture.idle();

    let (request, reply) = fixture.request("removed-after-request");
    fixture
        .app
        .world_mut()
        .get_component_mut::<Tools>(fixture.agent)
        .unwrap()
        .remove("second");
    fixture.run(request, reply).unwrap();
    let (_, _, body) = server
        .received
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let definitions = body["tools"].as_array().unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0]["function"]["name"], "visible");
}

#[test]
fn tool_result_images_keep_each_formats_native_or_deferred_layout() {
    for (provider, format) in [
        ("openai", "chat_completions"),
        ("anthropic", "messages"),
        ("openai", "responses"),
    ] {
        let server = HttpServer::new(trace(format));
        let mut fixture = Fixture::new(&server.url, provider, format);
        let (mut request, reply) = fixture.request("images");
        request.messages.push(InferenceMessage::Assistant {
            content: None,
            reasoning: None,
            tool_calls: vec![InferenceToolCall {
                id: "call".into(),
                name: "echo".into(),
                arguments: json!({}),
            }],
        });
        request.messages.push(InferenceMessage::Tool {
            call_id: "call".into(),
            content: "result".into(),
            images: vec![InferenceImage {
                media_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            }],
            failed: false,
        });
        fixture.run(request, reply).unwrap();
        let (_, _, body) = server
            .received
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        match format {
            "chat_completions" => {
                assert_eq!(body["messages"][2]["role"], "tool");
                assert_eq!(body["messages"][3]["role"], "user");
                assert_eq!(body["messages"][3]["content"][1]["type"], "image_url");
            }
            "messages" => {
                assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
                assert_eq!(
                    body["messages"][2]["content"][0]["content"][1]["type"],
                    "image"
                );
            }
            "responses" => {
                assert_eq!(body["input"][2]["type"], "function_call_output");
                assert_eq!(body["input"][2]["output"][1]["type"], "input_image");
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn invalid_turn_and_stale_entity_never_find_a_replacement_by_address() {
    let mut fixture = Fixture::new("http://127.0.0.1:1/v1", "openai", "chat_completions");
    let (mut request, reply) = fixture.request("wrong-turn");
    request.turn = Some("old".into());
    assert_eq!(
        fixture.run(request, reply).unwrap().stop_reason,
        InferenceStopReason::Completed
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<ActiveInference>(fixture.agent)
            .unwrap()
            .current
            .is_none()
    );
    let (request, reply) = fixture.request("old-instance");
    let original = fixture.agent;
    fixture.app.world_mut().despawn(original);
    let replacement = fixture.app.world_mut().spawn();
    assert_eq!(original.index(), replacement.index());
    assert_ne!(original.generation(), replacement.generation());
    fixture.app.world_mut().insert_component(
        replacement,
        Address::new("agent:margatroid-test-dev/agent:dev").unwrap(),
    );
    assert_eq!(
        fixture.run(request, reply).unwrap_err().kind,
        Kind::AgentMissing
    );
    assert!(
        !fixture
            .app
            .world()
            .contains_component::<ActiveInference>(replacement)
    );
}

#[test]
fn route_configuration_rejects_unknown_combinations_options_and_secret_urls() {
    let root = tempfile::tempdir().unwrap();
    let path: PathBuf = root.path().join("models.toml");
    for (provider, format, extra) in [
        ("openai", "messages", ""),
        ("anthropic", "responses", ""),
        ("openrouter", "responses", ""),
        ("unknown", "chat_completions", ""),
        ("openai", "chat_completions", "unknown_option=true"),
        (
            "openai",
            "chat_completions",
            "thinking='enabled'\nreasoning_effort='high'",
        ),
    ] {
        std::fs::write(&path, format!("[[models]]\nid='route'\nmodel='model'\nprovider='{provider}'\napi_type='{format}'\nbase_url='http://127.0.0.1:1/v1'\napi_key='SECRET-KEY'\n{extra}\n")).unwrap();
        let error = InferencePlugin::open(&path).err().unwrap();
        assert_eq!(error.kind, Kind::InvalidModelRoute);
        assert!(!error.message.contains("SECRET-KEY"));
    }
    std::fs::write(&path, "[[models]]\nid='route'\nmodel='model'\nprovider='openai'\napi_type='chat_completions'\nbase_url='https://username:SECRET-PASSWORD@example.invalid/v1'\napi_key='SECRET-KEY'\n").unwrap();
    let error = InferencePlugin::open(&path).err().unwrap();
    assert_eq!(error.kind, Kind::InvalidModelRoute);
    assert!(!error.message.contains("SECRET-PASSWORD"));
}

fn install_vm(world: &mut core_plugin::World, owner: Entity, start_id: &str) {
    world.insert_component(
        owner,
        lua_runtime_core::DriverLuaVm {
            instance: lua_runtime_core::DriverLuaVmInstance {
                owner,
                start_id: start_id.into(),
            },
            wake: lua_runtime_core::DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(None)),
            ended: None,
            cancel: CancellationToken::new(),
        },
    );
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn next_ws(
    fixture: &mut Fixture,
    runtime: &tokio::runtime::Runtime,
    socket: &mut Socket,
    expected: impl Fn(&Value) -> bool,
) -> Value {
    use futures_util::StreamExt;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        fixture.app.tick();
        if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) = runtime
            .block_on(async { tokio::time::timeout(Duration::from_millis(5), socket.next()).await })
        {
            let value: Value = serde_json::from_str(&text).unwrap();
            if expected(&value) {
                return value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "expected DTO or inference websocket frame did not arrive"
        );
    }
}

#[test]
fn address_only_state_and_streams_preserve_target_through_vm_restart() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let trace = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":{"reasoning_content":"thinking","content":"answer"},"finish_reason":"stop"}]})
    );
    let server = HttpServer::new(trace);
    let mut fixture = Fixture::with_dto(&server.url);
    let storage = storage_core::open_storage(
        &fixture.root.path().join("history.sqlite"),
        &fixture.root.path().join("settings.toml"),
        "",
    )
    .unwrap();
    fixture
        .app
        .world_mut()
        .insert_component(fixture.agent, storage.clone());
    let waits =
        agent_core::notify_agent_runtime(fixture.app.world_mut(), fixture.agent, false).unwrap();
    assert!(waits.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    let address = loop {
        fixture.app.tick();
        if let Some(address) = fixture
            .app
            .world()
            .get_resource::<axum_plugin::AxumHandle>()
            .and_then(axum_plugin::AxumHandle::local_address)
        {
            break address;
        }
        assert!(
            Instant::now() < deadline,
            "websocket listener did not start"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (mut socket, _) = runtime
        .block_on(tokio_tungstenite::connect_async(format!(
            "ws://{address}/ws"
        )))
        .unwrap();
    runtime.block_on(socket.send(Message::Text(json!({"type":"connection.register","id":"register","message":{"client_type":"webui","name":"browser"}}).to_string().into()))).unwrap();
    next_ws(&mut fixture, &runtime, &mut socket, |frame| {
        frame["type"] == "connection.register.result" && frame["success"] == true
    });
    let mut expected_instance = "driver-fixture";
    for (request_id, instance) in [
        ("first-stream", "driver-fixture"),
        ("after-vm-restart", "driver-restarted"),
    ] {
        if instance != expected_instance {
            install_vm(fixture.app.world_mut(), fixture.agent, instance);
            expected_instance = instance;
        }
        if request_id == "first-stream" {
            let state = next_ws(&mut fixture, &runtime, &mut socket, |frame| {
                frame["type"] == "agent.sync"
            });
            assert!(
                state["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|agent| agent.get("instance").is_none())
            );
        }
        let (request, reply) = fixture.request(request_id);
        fixture.app.world().emit_event(request);
        fixture.app.tick();
        let response = fixture.response(reply).unwrap();
        assert_eq!(response.content.as_deref(), Some("answer"));
        let reasoning = next_ws(&mut fixture, &runtime, &mut socket, |frame| {
            frame["generation"] == request_id
                && frame["type"] == "agent.message.chunk.sync"
                && frame["reasoning"] == "thinking"
        });
        let content = next_ws(&mut fixture, &runtime, &mut socket, |frame| {
            frame["generation"] == request_id
                && frame["type"] == "agent.message.chunk.sync"
                && frame["content"] == "answer"
        });
        let finished = next_ws(&mut fixture, &runtime, &mut socket, |frame| {
            frame["generation"] == request_id && frame["type"] == "agent.message.full.sync"
        });
        for frame in [&reasoning, &content, &finished] {
            assert!(frame.get("instance").is_none());
            assert_eq!(frame["address"], "agent:margatroid-test-dev/agent:dev");
            serde_json::from_value::<margatroid_protocol::ServerMessage>(frame.clone()).unwrap();
        }
        assert_eq!(reasoning["reasoning"], "thinking");
        assert_eq!(reasoning["sequence"], 1);
        assert_eq!(content["content"], "answer");
        assert_eq!(content["sequence"], 2);
        assert!(content.get("delta").is_none());
        assert_eq!(content["reasoning"], "thinking");
        assert!(finished.get("sequence").is_none());
        assert_eq!(finished["error"], Value::Null);
        storage_core::execute_storage(
            &storage,
            storage_core::StorageOperation::AppendHistory {
                records: vec![
                    storage_core::HistoryRecord {
                        source: None,
                        turn: Some("turn".into()),
                        kind: "message".into(),
                        content: "answer".into(),
                        payload: finished["message"].clone(),
                        created_at_ms: None,
                    },
                    storage_core::HistoryRecord {
                        source: None,
                        turn: Some("turn".into()),
                        kind: "message".into(),
                        content: format!("persisted-{request_id}"),
                        payload: json!({"type":"user","content":format!("persisted-{request_id}")}),
                        created_at_ms: None,
                    },
                ],
            },
        )
        .unwrap();
        // The following user record is a same-batch synchronization barrier.
        // A duplicate assistant full would precede it and must fail this test.
        next_ws(&mut fixture, &runtime, &mut socket, |frame| {
            assert!(
                !(frame["type"] == "agent.message.full.sync" && frame["generation"] == request_id),
                "persisting a generated assistant emitted a second full: {frame}"
            );
            frame["type"] == "agent.message.full.sync"
                && frame["message"]["content"] == format!("persisted-{request_id}")
        });
        let rows = storage_core::read_message_page(
            &storage,
            storage_core::RecordRange {
                after_sequence: None,
                before_sequence: None,
                limit: 50,
            },
        )
        .unwrap();
        assert!(
            rows.records
                .iter()
                .any(|row| row.record.payload["request_id"] == request_id
                    && row.record.payload["sequence"].as_u64() == Some(row.sequence))
        );
        fixture.idle();
    }
    runtime.block_on(socket.close(None)).unwrap();
}

#[test]
fn inference_uses_agent_lifetime_without_requiring_or_interpreting_a_driver_vm() {
    let server = HttpServer::new(format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":{"content":"independent"},"finish_reason":"stop"}]})
    ));
    let mut fixture = Fixture::new(&server.url, "openai", "chat_completions");
    for variant in ["no-vm", "unrelated-vm"] {
        fixture
            .app
            .world_mut()
            .remove_component::<lua_runtime_core::DriverLuaVm>(fixture.agent);
        if variant == "unrelated-vm" {
            install_vm(fixture.app.world_mut(), fixture.agent, "not-the-owner");
            let other = fixture.app.world_mut().spawn();
            fixture
                .app
                .world_mut()
                .get_component_mut::<lua_runtime_core::DriverLuaVm>(fixture.agent)
                .unwrap()
                .instance
                .owner = other;
        }
        let (request, reply) = fixture.request(variant);
        assert_eq!(
            fixture.run(request, reply).unwrap().content.as_deref(),
            Some("independent")
        );
        fixture.idle();
    }
}

fn historical_reasoning_fixture() -> Vec<InferenceMessage> {
    vec![
        InferenceMessage::User {
            content: "previous question".into(),
            images: vec![],
        },
        InferenceMessage::Assistant {
            reasoning: Some("local historical reasoning stays intact".into()),
            content: Some("previous answer".into()),
            tool_calls: vec![InferenceToolCall {
                id: "prior-call".into(),
                name: "visible".into(),
                arguments: json!({}),
            }],
        },
        InferenceMessage::Tool {
            call_id: "prior-call".into(),
            content: "previous tool result".into(),
            images: vec![],
            failed: false,
        },
        InferenceMessage::User {
            content: "continue".into(),
            images: vec![],
        },
    ]
}

#[test]
fn historical_reasoning_is_format_adapted_without_rewriting_local_messages() {
    for (provider, format) in [
        ("openai", "chat_completions"),
        ("openai", "responses"),
        ("anthropic", "messages"),
        ("deepseek", "chat_completions"),
        ("deepseek", "messages"),
        ("deepseek", "responses"),
        ("openrouter", "chat_completions"),
    ] {
        let server = HttpServer::new(trace(format));
        let mut fixture = Fixture::new(&server.url, provider, format);
        fixture.tool("historical visible tool");
        for index in 0..3 {
            let messages = historical_reasoning_fixture();
            let snapshot = messages.clone();
            let (mut request, reply) = fixture.request(&format!("history-{index}"));
            request.messages = messages.clone();

            let response = fixture
                .run(request, reply)
                .unwrap_or_else(|e| panic!("{provider}/{format}: {e}"));
            assert_eq!(response.content.as_deref(), Some("ok"));
            fixture.idle();
            let (_, _, body) = server
                .received
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            let encoded = body.to_string();
            assert!(encoded.contains("previous answer"), "{body}");
            assert!(encoded.contains("previous tool result"), "{body}");
            assert!(encoded.contains("prior-call"), "{body}");
            assert!(encoded.contains("continue"), "{body}");
            assert_eq!(messages, snapshot);
            assert!(!encoded.contains("signature"));
            assert!(!encoded.contains("encrypted_content"));
            if format != "chat_completions" || provider == "openai" {
                assert!(!encoded.contains("local historical reasoning stays intact"));
            } else {
                assert!(encoded.contains("local historical reasoning stays intact"));
            }
        }
    }
}

#[test]
fn new_response_reasoning_remains_generic_across_storage_and_api_switches() {
    for (provider, format, events, marker) in [
        (
            "anthropic",
            "messages",
            vec![
                json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"thought"}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"roundtrip-signature"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"answer"}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"}),
            ],
            "roundtrip-signature",
        ),
        (
            "deepseek",
            "responses",
            vec![
                json!({"type":"response.created"}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"roundtrip-reason","summary":[],"encrypted_content":"roundtrip-encrypted"}}),
                json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"thought"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"roundtrip-reason","summary":[{"type":"summary_text","text":"thought"}]}}),
                json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","content":[]}}),
                json!({"type":"response.output_text.delta","output_index":1,"delta":"answer"}),
                json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"answer"}]}}),
                json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":2}}}),
            ],
            "roundtrip-encrypted",
        ),
    ] {
        let response_trace = events
            .into_iter()
            .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
            .collect();
        let first_server = HttpServer::new(response_trace);
        let mut first = Fixture::new(&first_server.url, provider, format);
        let (request, reply) = first.request("first-round");
        let result = first.run(request, reply).unwrap();
        first.idle();
        assert_eq!(result.reasoning.as_deref(), Some("thought"));
        assert!(!serde_json::to_string(&result).unwrap().contains(marker));
        let original = InferenceMessage::Assistant {
            reasoning: result.reasoning,
            content: result.content,
            tool_calls: result.tool_calls,
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let storage_root = tempfile::tempdir().unwrap();
        let storage = storage_core::open_storage(
            &storage_root.path().join("messages.sqlite"),
            &storage_root.path().join("settings.toml"),
            "",
        )
        .unwrap();
        storage_core::execute_storage(
            &storage,
            storage_core::StorageOperation::CreateBlock {
                name: "session".into(),
                fields: vec!["conversation".into()],
                instance: None,
            },
        )
        .unwrap();
        let selector = storage_core::MessageSelector {
            block: "session".into(),
            field: "conversation".into(),
            instance: None,
            selection: storage_core::MessageSelection::All,
        };
        storage_core::execute_storage(
            &storage,
            storage_core::StorageOperation::ReplaceMessages {
                sources: vec![storage_core::MessageSource::Literal(vec![
                    serde_json::to_value(&original).unwrap(),
                ])],
                target: selector.clone(),
            },
        )
        .unwrap();
        let storage_core::StorageValue::Messages(stored) = storage_core::execute_storage(
            &storage,
            storage_core::StorageOperation::ReadMessages {
                selectors: vec![selector],
            },
        )
        .unwrap() else {
            panic!("stored message")
        };
        let restored: InferenceMessage = serde_json::from_value(stored[0].clone()).unwrap();
        assert_eq!(original, restored);
        for target in [
            format,
            if format == "messages" {
                "responses"
            } else {
                "messages"
            },
        ] {
            let server = HttpServer::new(trace(target));
            let mut second = Fixture::new(&server.url, "deepseek", target);
            let (mut request, reply) = second.request("second-round");
            request.messages.push(restored.clone());
            request.messages.push(InferenceMessage::User {
                content: "next".into(),
                images: vec![],
            });
            second.run(request, reply).unwrap();
            let (_, _, body) = server
                .received
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            assert!(body.to_string().contains("answer"));
            assert!(!body.to_string().contains(marker));
            assert!(!body.to_string().contains("thought"));
            assert_eq!(serde_json::to_string(&original).unwrap(), serialized);
        }
    }
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}

fn test_probe<T>() -> client_core::UpdateProbe<T> {
    client_core::UpdateProbe::new(|_| {})
}

#[test]
fn free_inference_executes_without_a_turn_in_starting_and_running_and_never_inherits_one() {
    use agent_core::WorldAgentExt;
    for (state, current) in [
        (AgentState::Starting, None),
        (AgentState::Running, None),
        (AgentState::Running, Some("unrelated-turn".to_owned())),
    ] {
        let server = HttpServer::new(trace("chat_completions"));
        let mut fixture = Fixture::new(&server.url, "openai", "chat_completions");
        fixture
            .app
            .world_mut()
            .agent_transition(fixture.agent, state, current.clone())
            .unwrap();
        let (mut request, reply) = fixture.request("free-inference");
        request.turn = None;
        let result = fixture.run(request, reply).unwrap();
        assert!(result.content.is_some());
        fixture.idle();
        assert!(server.received.recv_timeout(Duration::from_secs(1)).is_ok());
        let handle = fixture
            .app
            .world()
            .get_component::<ActiveInference>(fixture.agent)
            .unwrap()
            .current
            .as_ref()
            .unwrap();
        assert_eq!(handle.turn, None);
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<agent_core::Agent>(fixture.agent)
                .unwrap()
                .turn(),
            &current
        );
    }
}
#[test]
fn free_inference_still_rejects_error_stopping_and_precancelled_requests() {
    use agent_core::WorldAgentExt;
    for (state, cancelled) in [
        (AgentState::Error, false),
        (AgentState::Stopping, false),
        (AgentState::Starting, true),
    ] {
        let server = HttpServer::new(trace("chat_completions"));
        let mut fixture = Fixture::new(&server.url, "openai", "chat_completions");
        fixture
            .app
            .world_mut()
            .agent_transition(fixture.agent, state, None)
            .unwrap();
        let (mut request, reply) = fixture.request("rejected-free");
        request.turn = None;
        if cancelled {
            request.cancel.cancel();
        }
        fixture.run(request, reply).unwrap();
        assert!(server.received.try_recv().is_err());
        assert!(
            fixture
                .app
                .world()
                .get_component::<ActiveInference>(fixture.agent)
                .unwrap()
                .current
                .is_none()
        );
    }
}

#[test]
fn each_inference_reads_the_model_file_instead_of_the_startup_routes() {
    let first = HttpServer::new(trace("chat_completions"));
    let mut fixture = Fixture::new(&first.url, "openai", "chat_completions");
    let file = fixture.root.path().join("models.toml");
    let (request, reply) = fixture.request("before-edit");
    fixture.run(request, reply).unwrap();
    fixture.idle();
    let (_, headers, body) = first.received.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(body["model"], "upstream-model");
    assert!(headers.contains("private-test-key"));
    let second = HttpServer::new(trace("chat_completions"));
    std::fs::write(&file,format!("[[models]]\nid='route'\nmodel='updated-model'\nprovider='openai'\napi_type='chat_completions'\nbase_url='{}'\napi_key='updated-key'\ncontext_window='256k'\n",second.url)).unwrap();
    let (request, reply) = fixture.request("after-edit");
    fixture.run(request, reply).unwrap();
    fixture.idle();
    let (_, headers, body) = second
        .received
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(body["model"], "updated-model");
    assert!(headers.contains("updated-key"));
    assert!(!headers.contains("private-test-key"));
    std::fs::write(&file, "invalid toml [").unwrap();
    let (request, reply) = fixture.request("malformed-file");
    let failed = fixture.run(request, reply).unwrap_err();
    assert_eq!(failed.kind, Kind::InvalidModelRoute);
    fixture.idle();
    std::fs::write(&file,format!("[[models]]\nid='different-id'\nmodel='other-model'\nprovider='openai'\napi_type='chat_completions'\nbase_url='{}'\napi_key='key'\n",second.url)).unwrap();
    let (request, reply) = fixture.request("removed-model-id");
    assert_eq!(
        fixture.run(request, reply).unwrap_err().kind,
        Kind::ModelRouteMissing
    );
    fixture.idle();
    std::fs::remove_file(&file).unwrap();
    let (request, reply) = fixture.request("missing-model-file");
    assert_eq!(
        fixture.run(request, reply).unwrap_err().kind,
        Kind::InvalidModelRoute
    );
}

fn model_frame(
    fixture: &mut Fixture,
    runtime: &tokio::runtime::Runtime,
    socket: &mut Socket,
    seen: &mut Vec<Value>,
    expected: impl Fn(&Value) -> bool,
) -> Value {
    use futures_util::StreamExt;
    if let Some(value) = seen.iter().find(|value| expected(value)) {
        return value.clone();
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        fixture.app.tick();
        if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) = runtime
            .block_on(async { tokio::time::timeout(Duration::from_millis(5), socket.next()).await })
        {
            let value: Value = serde_json::from_str(&text).unwrap();
            seen.push(value.clone());
            if expected(&value) {
                return value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "model configuration update did not arrive: {seen:?}"
        );
    }
}
#[test]
fn model_configuration_api_reads_replaces_and_syncs_external_edits_without_agent_or_turn() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let mut fixture = Fixture::with_dto("http://127.0.0.1:1/v1");
    let file = fixture.root.path().join("models.toml");
    let original = std::fs::read_to_string(&file).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let address = loop {
        fixture.app.tick();
        if let Some(address) = fixture
            .app
            .world()
            .get_resource::<axum_plugin::AxumHandle>()
            .unwrap()
            .local_address()
        {
            break address;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (mut socket, _) = runtime
        .block_on(tokio_tungstenite::connect_async(format!(
            "ws://{address}/ws"
        )))
        .unwrap();
    let mut seen = Vec::new();
    let send = |socket: &mut Socket, kind: &str, id: &str, message: Value| {
        runtime
            .block_on(
                socket.send(Message::Text(
                    json!({"type":kind,"id":id,"message":message})
                        .to_string()
                        .into(),
                )),
            )
            .unwrap()
    };
    send(
        &mut socket,
        "model.config.update",
        "unregistered",
        json!({"content":"invalid"}),
    );
    let denied = model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| {
        v["id"] == "unregistered"
    });
    assert_eq!(denied["success"], false);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    send(
        &mut socket,
        "connection.register",
        "register-models",
        json!({"client_type":"webui","name":"models-editor"}),
    );
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "register-models")["success"],
        true
    );
    send(&mut socket, "model.config.req", "read-models", json!({}));
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "read-models")["content"],
        original
    );
    send(
        &mut socket,
        "model.config.update",
        "invalid-models",
        json!({"content":"[broken"}),
    );
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "invalid-models")["success"],
        false
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    let replacement = original
        .replace("upstream-model", "configured-model")
        .replace("private-test-key", "new-secret");
    send(
        &mut socket,
        "model.config.update",
        "replace-models",
        json!({"content":replacement}),
    );
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "replace-models")["success"],
        true
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), replacement);
    model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| {
        v["type"] == "model.config.sync" && v["content"] == replacement
    });
    assert_eq!(
        fixture
            .app
            .world()
            .get_resource::<inference_core::ModelRoutes>()
            .unwrap()
            .get("route")
            .unwrap()
            .model,
        "configured-model"
    );
    let external = replacement.replace("configured-model", "externally-edited-model");
    let temporary = file.with_file_name("models-edited.toml");
    std::fs::write(&temporary, &external).unwrap();
    std::fs::rename(&temporary, &file).unwrap();
    model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| {
        v["type"] == "model.config.sync" && v["content"] == external
    });
    std::fs::write(&file, "malformed external file [").unwrap();
    model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| {
        v["type"] == "model.config.sync" && v["content"] == "malformed external file ["
    });
    send(&mut socket, "model.config.req", "read-malformed", json!({}));
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "read-malformed")["content"],
        "malformed external file ["
    );
    std::fs::remove_file(&file).unwrap();
    model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| {
        v["type"] == "model.config.sync" && v["content"].is_null() && v["error"].is_object()
    });
    std::fs::write(&file, &external).unwrap();
    send(&mut socket, "model.config.req", "restored", json!({}));
    assert_eq!(
        model_frame(&mut fixture, &runtime, &mut socket, &mut seen, |v| v["id"]
            == "restored")["content"],
        external
    );
    runtime.block_on(socket.close(None)).unwrap();
}
