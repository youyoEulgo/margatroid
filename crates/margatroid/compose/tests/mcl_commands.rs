mod common;

use agent_core::{Agent, AgentState, WorldAgentExt};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, World};
use inference_core::*;
use lua_runtime_core::*;
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_core::*;
use mcl_plugin::{MclImportConfig, MclPlugin};
use prompt_resource_core::{ImportPrompt, Prompts};
use sandbox_resource_core::{ImportSandbox, Sandboxes, SelectedSandbox};
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage_core::{AgentStorage, StorageOperation, StorageValue, execute_storage, open_storage};
use storage_plugin::StoragePlugin;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_core::{
    ToolExecuteRequest, ToolExecuteResponse, ToolExecutionLimits, ToolTaskHandle, ToolTasks,
};
use tool_resource_core::{ImportTool, Tools};

trait ErrorValueExt {
    fn error_kind(self) -> MclErrorKind;
}

impl ErrorValueExt for Result<MclValue, MclError> {
    fn error_kind(self) -> MclErrorKind {
        let MclValue::Error(message) = self.expect("MCL failures must return values") else {
            panic!("expected an Error value")
        };
        assert_eq!(message["type"], "error");
        assert!(message["request_id"].is_string());
        let kind = message["message"]
            .as_str()
            .unwrap()
            .split_once(':')
            .unwrap()
            .0;
        serde_json::from_value(json!(kind)).unwrap()
    }
}

struct Fixture {
    app: App,
    owner: Entity,
    client: Entity,
    root: tempfile::TempDir,
    next: usize,
    inference: Arc<Mutex<Vec<InferenceRequest>>>,
    auto_inference: Arc<AtomicBool>,
    tools: Arc<Mutex<Vec<ToolExecuteRequest>>>,
    tool_error: Arc<Mutex<Option<tool_core::ToolError>>>,
    tool_modes: Arc<Mutex<std::collections::BTreeMap<String, u8>>>,
    imports: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let roots = MclImportConfig {
            prompt_root: root.path().join("prompts"),
            tool_root: root.path().join("tools"),
            sandbox_root: root.path().join("sandboxes"),
        };
        for path in [&roots.prompt_root, &roots.tool_root, &roots.sandbox_root] {
            fs::create_dir_all(path).unwrap();
        }
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(StoragePlugin::new())
            .add_plugin(MclPlugin::new().with_import_config(roots));
        let workspace = app.world_mut().spawn();
        let owner = app.world_mut().spawn();
        let client = app.world_mut().spawn();
        app.world_mut().insert_component(
            owner,
            address::Address::new("agent:local/test:latest").unwrap(),
        );
        app.world_mut().insert_component(
            client,
            address::Address::new("client:local/frontend:latest").unwrap(),
        );
        let storage = open_storage(
            &root.path().join("agent.sqlite"),
            &root.path().join("settings.toml"),
            "[compaction]\nthreshold = 0.8\n",
        )
        .unwrap();
        app.world_mut().insert_component(owner, storage);
        app.world_mut().insert_component(
            owner,
            Agent {
                name: "test".into(),
                workspace,
                state: AgentState::Idle,
                turn: None,
            },
        );
        app.world_mut().insert_component(owner, MclQueue::default());
        app.world_mut().insert_component(
            owner,
            AgentInferenceConfig {
                parameters: InferenceParameters {
                    model: "test/model".into(),
                    temperature: Some(0.1),
                    max_output_tokens: None,
                    top_p: None,
                    stop: vec![],
                },
            },
        );
        app.world_mut()
            .insert_component(owner, ActiveInference::default());
        app.world_mut().insert_component(owner, ToolTasks::new());
        let inference = Arc::new(Mutex::new(Vec::new()));
        let recorded = inference.clone();
        let auto_inference = Arc::new(AtomicBool::new(true));
        let automatic = auto_inference.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            let requests = world
                .event_reader::<InferenceRequest>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            for request in requests {
                recorded.lock().unwrap().push(request.clone());
                if automatic.load(Ordering::Acquire)
                    && let Some(sender) = request.reply.lock().unwrap().take()
                {
                    let _ = sender.send(Ok(response()));
                }
            }
        });
        let tools = Arc::new(Mutex::new(Vec::new()));
        let tool_error = Arc::new(Mutex::new(None::<tool_core::ToolError>));
        let configured_error = tool_error.clone();
        let tool_modes = Arc::new(Mutex::new(std::collections::BTreeMap::<String, u8>::new()));
        let configured_modes = tool_modes.clone();
        let recorded = tools.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            let requests = world
                .event_reader::<ToolExecuteRequest>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            for request in requests {
                recorded.lock().unwrap().push(request.clone());
                if let Some(mode) = configured_modes
                    .lock()
                    .unwrap()
                    .get(&request.call_id)
                    .copied()
                {
                    let sender = request.reply.lock().unwrap().take().unwrap();
                    match mode {
                        1 | 2 => {
                            let _ = sender.send(Ok(ToolExecuteResponse {
                                call_id: if mode == 1 {
                                    "wrong-call".into()
                                } else {
                                    request.call_id.clone()
                                },
                                alias: if mode == 2 {
                                    "wrong-alias".into()
                                } else {
                                    request.alias.clone()
                                },
                                content: "untrusted response".into(),
                                images: vec![],
                                failed: false,
                            }));
                        }
                        3 => drop(sender),
                        4 => {
                            let _ = sender.send(Err(tool_core::ToolError::new(
                                tool_core::ToolErrorKind::CleanupFailed,
                                "cleanup not proven",
                            )));
                        }
                        5 => {
                            let (handle, _input) = ToolTaskHandle::new(
                                &ToolExecutionLimits::default(),
                                request.call_id.clone(),
                                request.alias.clone(),
                                sender,
                            )
                            .unwrap();
                            world
                                .get_component::<ToolTasks>(request.agent)
                                .unwrap()
                                .register(request.id.clone(), handle)
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                    continue;
                }
                if let Some(error) = configured_error.lock().unwrap().clone() {
                    if let Some(sender) = request.reply.lock().unwrap().take() {
                        let _ = sender.send(Err(error));
                    }
                    continue;
                }
                let sender = request.reply.lock().unwrap().take().unwrap();
                let (handle, _input) = ToolTaskHandle::new(
                    &ToolExecutionLimits::default(),
                    request.call_id.clone(),
                    request.alias.clone(),
                    sender,
                )
                .unwrap();
                handle
                    .return_result(Ok(ToolExecuteResponse {
                        call_id: request.call_id,
                        alias: request.alias,
                        content: request.arguments.to_string(),
                        images: vec![],
                        failed: false,
                    }))
                    .unwrap();
                world
                    .get_component::<ToolTasks>(request.agent)
                    .unwrap()
                    .register(request.id.clone(), handle)
                    .unwrap();
            }
        });
        let imports = Arc::new(Mutex::new(Vec::new()));
        let recorded = imports.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            for request in world
                .event_reader::<ImportPrompt>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                recorded
                    .lock()
                    .unwrap()
                    .push(format!("prompt:{}", request.alias));
                if !world.contains_component::<Prompts>(request.owner) {
                    world.insert_component(request.owner, Prompts::new());
                }
                world
                    .get_component_mut::<Prompts>(request.owner)
                    .unwrap()
                    .insert(request.alias, request.path);
                if let Some(sender) = request.reply.lock().unwrap().take() {
                    let _ = sender.send(Ok(()));
                }
            }
            for request in world
                .event_reader::<ImportTool>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                recorded
                    .lock()
                    .unwrap()
                    .push(format!("tool:{}", request.alias));
                if !world.contains_component::<Tools>(request.owner) {
                    world.insert_component(request.owner, Tools::new());
                }
                world
                    .get_component_mut::<Tools>(request.owner)
                    .unwrap()
                    .insert(request.alias, common::tool(request.path))
                    .unwrap();
                if let Some(sender) = request.reply.lock().unwrap().take() {
                    let _ = sender.send(Ok(()));
                }
            }
            for request in world
                .event_reader::<ImportSandbox>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                recorded
                    .lock()
                    .unwrap()
                    .push(format!("sandbox:{}", request.alias));
                if !world.contains_component::<Sandboxes>(request.owner) {
                    world.insert_component(request.owner, Sandboxes::new());
                }
                world
                    .get_component_mut::<Sandboxes>(request.owner)
                    .unwrap()
                    .insert(request.alias, request.path);
                if let Some(sender) = request.reply.lock().unwrap().take() {
                    let _ = sender.send(Ok(()));
                }
            }
        });
        Self {
            app,
            owner,
            client,
            root,
            next: 0,
            inference,
            auto_inference,
            tools,
            tool_error,
            tool_modes,
            imports,
        }
    }

    fn enqueue(
        &mut self,
        text: &str,
        arguments: Vec<Value>,
    ) -> oneshot::Receiver<Result<MclValue, MclError>> {
        self.next += 1;
        self.enqueue_as(
            format!("command-{}", self.next),
            self.client,
            None,
            self.owner,
            text,
            arguments,
        )
    }

    fn enqueue_as(
        &self,
        id: String,
        source: Entity,
        source_instance: Option<String>,
        target: Entity,
        text: &str,
        arguments: Vec<Value>,
    ) -> oneshot::Receiver<Result<MclValue, MclError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().send_event(MclCommand {
            id,
            source,
            source_address: self
                .app
                .world()
                .get_component::<address::Address>(source)
                .unwrap()
                .as_str()
                .to_owned(),
            source_instance,
            target,
            instruction: MclInstruction {
                text: text.into(),
                arguments,
            },
            cancel: CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }

    async fn command(&mut self, text: &str, arguments: Vec<Value>) -> Result<MclValue, MclError> {
        let mut receiver = self.enqueue(text, arguments);
        drive(&mut self.app, &mut receiver).await
    }

    fn vm(&mut self, id: &str) -> oneshot::Receiver<Result<(), LuaRuntimeError>> {
        let (sender, ready) = oneshot::channel();
        let (_sender, ended) = oneshot::channel();
        self.app.world_mut().insert_component(
            self.owner,
            DriverLuaVm {
                instance: DriverLuaVmInstance {
                    owner: self.owner,
                    start_id: id.into(),
                },
                wake: DriverWakeSlot::shared(),
                ready: Arc::new(Mutex::new(Some(sender))),
                ended: Some(ended),
                cancel: CancellationToken::new(),
            },
        );
        ready
    }

    fn add_tool(&mut self, alias: &str) {
        if !self.app.world().contains_component::<Tools>(self.owner) {
            self.app
                .world_mut()
                .insert_component(self.owner, Tools::new());
        }
        let directory = self.root.path().join("fixture-tool").join(alias);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("tool.toml"),
            "schema_version=1\ndescription='fixture tool'\n",
        )
        .unwrap();
        fs::write(directory.join("input.schema.json"), "{\"type\":\"object\"}").unwrap();
        fs::write(directory.join("main.lua"), "function execute() end").unwrap();
        self.app
            .world_mut()
            .get_component_mut::<Tools>(self.owner)
            .unwrap()
            .insert(alias.into(), common::tool(directory.join("tool.toml")))
            .unwrap();
    }

    fn history(&self) -> Vec<storage_core::HistoryRecord> {
        match execute_storage(
            self.app
                .world()
                .get_component::<AgentStorage>(self.owner)
                .unwrap(),
            StorageOperation::ReadHistory,
        )
        .unwrap()
        {
            StorageValue::History(records) => records,
            _ => panic!("history"),
        }
    }
}

async fn drive<T>(app: &mut App, receiver: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        match receiver.try_recv() {
            Ok(value) => return value,
            Err(oneshot::error::TryRecvError::Closed) => panic!("request reply was lost"),
            Err(oneshot::error::TryRecvError::Empty) => (),
        }
        assert!(Instant::now() < deadline, "request did not settle");
        tokio::task::yield_now().await;
    }
}

async fn tick_until(app: &mut App, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        app.tick();
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
}

fn response() -> InferenceResponse {
    InferenceResponse {
        reasoning: Some("reason".into()),
        content: Some("response".into()),
        tool_calls: vec![],
        replay: None,
        usage: None,
        stop_reason: InferenceStopReason::Completed,
    }
}

fn messages(value: MclValue) -> Vec<Value> {
    match value {
        MclValue::Array(MessageArray::Message(messages)) => messages,
        _ => panic!("message array"),
    }
}

fn user(text: &str) -> Value {
    json!({"type": "user", "content": text})
}

#[tokio::test]
async fn message_blocks_reopen_without_clearing_and_self_inject_is_transactional() {
    let mut fixture = Fixture::new();
    fixture
        .command("CREATE BLOCK session (conversation)", vec![])
        .await
        .unwrap();
    fixture
        .command(
            "INJECT ? TO session.conversation",
            vec![json!([user("first"), user("second")])],
        )
        .await
        .unwrap();
    fixture
        .command("CREATE BLOCK session (conversation)", vec![])
        .await
        .unwrap();
    assert_eq!(
        messages(
            fixture
                .command("GET session.conversation[-1]", vec![])
                .await
                .unwrap()
        ),
        [user("second")]
    );
    fixture
        .command(
            "INJECT session.conversation[0] TO session.conversation[-1]",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        messages(
            fixture
                .command("GET session.conversation", vec![])
                .await
                .unwrap()
        ),
        [user("first"), user("second"), user("first")]
    );
    assert_eq!(
        fixture
            .command(
                "INJECT session.conversation[0] TO session.conversation[99]",
                vec![]
            )
            .await
            .error_kind(),
        MclErrorKind::InvalidRange
    );
    assert_eq!(
        messages(
            fixture
                .command("GET session.conversation", vec![])
                .await
                .unwrap()
        )
        .len(),
        3
    );
    fixture
        .command("INJECT [] TO session.conversation[0,2]", vec![])
        .await
        .unwrap();
    assert_eq!(
        messages(
            fixture
                .command("GET session.conversation", vec![])
                .await
                .unwrap()
        ),
        [user("first")]
    );
    assert!(fixture.history().iter().all(|record| record.kind == "mcl"));
}

#[tokio::test]
async fn temporary_blocks_are_vm_scoped_and_permanent_blocks_are_fallbacks() {
    let mut fixture = Fixture::new();
    let _ready = fixture.vm("first");
    fixture
        .command("CREATE TEMP BLOCK entry (pending)", vec![])
        .await
        .unwrap();
    fixture
        .command("CREATE BLOCK session (conversation)", vec![])
        .await
        .unwrap();
    fixture
        .command(
            "INJECT ? TO entry.pending",
            vec![json!([user("temporary")])],
        )
        .await
        .unwrap();
    fixture
        .command(
            "INJECT ? TO session.conversation",
            vec![json!([user("persistent")])],
        )
        .await
        .unwrap();
    let _ready = fixture.vm("second");
    assert_eq!(
        fixture
            .command("GET entry.pending", vec![])
            .await
            .error_kind(),
        MclErrorKind::StorageFailed
    );
    fixture
        .command("CREATE TEMP BLOCK entry (pending)", vec![])
        .await
        .unwrap();
    assert!(messages(fixture.command("GET entry.pending", vec![]).await.unwrap()).is_empty());
    assert_eq!(
        messages(
            fixture
                .command("GET session.conversation", vec![])
                .await
                .unwrap()
        ),
        [user("persistent")]
    );
}

#[tokio::test]
async fn config_preserves_typed_values_and_failed_writes_leave_previous_file() {
    let mut fixture = Fixture::new();
    for (path, value) in [
        ("visible_tools", json!(["bash", "read_file"])),
        ("limits.tokens", json!(2048)),
        ("limits.enabled", json!(true)),
        ("limits.ratio", json!(0.5)),
    ] {
        fixture
            .command("CONFIG SET ? ?", vec![json!(path), value])
            .await
            .unwrap();
    }
    fixture
        .command(
            "CONFIG SET ? ?",
            vec![json!("literal_date"), json!("1979-05-27")],
        )
        .await
        .unwrap();
    let datetime = toml::Value::Datetime("1979-05-27T07:32:00Z".parse().unwrap());
    fixture
        .command(
            "CONFIG SET ? ?",
            vec![
                json!("typed_datetime"),
                serde_json::to_value(&datetime).unwrap(),
            ],
        )
        .await
        .unwrap();
    let MclValue::Config(config) = fixture.command("CONFIG GET", vec![]).await.unwrap() else {
        panic!("config");
    };
    assert_eq!(config["typed_datetime"], datetime);
    assert_eq!(config["limits"]["tokens"].as_integer(), Some(2048));
    assert_eq!(config["literal_date"].as_str(), Some("1979-05-27"));
    assert_eq!(
        fixture
            .command(
                "CONFIG SET ? ?",
                vec![json!("limits.tokens.child"), json!(1)]
            )
            .await
            .error_kind(),
        MclErrorKind::ConfigTypeMismatch
    );
    assert_eq!(
        fixture
            .command("CONFIG SET ? ?", vec![json!("null"), Value::Null])
            .await
            .error_kind(),
        MclErrorKind::ConfigTypeMismatch
    );
    fixture
        .command("CONFIG DELETE ?", vec![json!("limits.tokens")])
        .await
        .unwrap();
    fixture
        .command("CONFIG DELETE ?", vec![json!("missing.key")])
        .await
        .unwrap();
    let MclValue::Config(config) = fixture.command("CONFIG GET", vec![]).await.unwrap() else {
        panic!("config");
    };
    assert!(config["limits"].as_table().unwrap().get("tokens").is_none());
    assert_eq!(config["limits"]["enabled"].as_bool(), Some(true));
}

#[tokio::test]
async fn imports_validate_dependencies_kind_and_cross_category_aliases_and_reread_prompts() {
    let mut fixture = Fixture::new();
    let prompt = fixture.root.path().join("prompts/local/soul/latest");
    fs::create_dir_all(&prompt).unwrap();
    fs::write(prompt.join("prompt.toml"), "role = 'system'\n").unwrap();
    fs::write(prompt.join("content.md"), "original prompt").unwrap();
    let tool = fixture.root.path().join("tools/local/bash/latest");
    fs::create_dir_all(&tool).unwrap();
    fs::write(
        tool.join("tool.toml"),
        "schema_version = 1\ndescription = 'test tool'\n",
    )
    .unwrap();
    fs::write(tool.join("input.schema.json"), "{}").unwrap();
    fs::write(tool.join("main.lua"), "function configure(arguments, context) return {timeout_ms=30000} end\nfunction execute() margatroid.task:return_result('test') end").unwrap();
    let sandbox = fixture.root.path().join("sandboxes/local/policy/latest");
    fs::create_dir_all(&sandbox).unwrap();
    fs::write(sandbox.join("policy.json"), "{}").unwrap();
    fixture
        .app
        .world_mut()
        .get_component_mut::<MclQueue>(fixture.owner)
        .unwrap()
        .allowed_imports = vec![
        "prompt:local/soul".into(),
        "tool:local/bash:latest".into(),
        "sandbox:local/policy".into(),
    ];
    fixture
        .command("IMPORT MESSAGE prompt:local/soul:latest AS soul", vec![])
        .await
        .unwrap();
    assert_eq!(
        fixture
            .command("IMPORT TOOL tool:local/bash AS soul", vec![])
            .await
            .error_kind(),
        MclErrorKind::AliasConflict
    );
    assert_eq!(
        fixture
            .command("IMPORT TOOL prompt:local/soul AS other", vec![])
            .await
            .error_kind(),
        MclErrorKind::ImportDenied
    );
    assert_eq!(
        fixture
            .command("IMPORT TOOL tool:local/undeclared AS other", vec![])
            .await
            .error_kind(),
        MclErrorKind::ImportDenied
    );
    fixture
        .command("IMPORT TOOL tool:local/bash AS bash", vec![])
        .await
        .unwrap();
    fixture
        .command("IMPORT SANDBOX sandbox:local/policy AS safe", vec![])
        .await
        .unwrap();
    fixture
        .command("EFFECT sandbox_use ?", vec![json!(["safe"])])
        .await
        .unwrap();
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<SelectedSandbox>(fixture.owner)
            .unwrap()
            .alias
            .as_deref(),
        Some("safe")
    );
    fixture
        .command("EFFECT sandbox_use ?", vec![json!([])])
        .await
        .unwrap();
    assert!(
        fixture
            .app
            .world()
            .get_component::<SelectedSandbox>(fixture.owner)
            .unwrap()
            .alias
            .is_none()
    );
    fixture
        .command("CREATE BLOCK session (conversation)", vec![])
        .await
        .unwrap();
    fixture
        .command(
            "INJECT ? TO session.conversation",
            vec![json!([user("stored")])],
        )
        .await
        .unwrap();
    assert_eq!(
        messages(
            fixture
                .command(
                    "GET session.conversation, soul, session.conversation",
                    vec![]
                )
                .await
                .unwrap()
        ),
        [
            user("stored"),
            json!({"type": "system", "content": "original prompt"}),
            user("stored")
        ]
    );
    fs::write(prompt.join("content.md"), "updated prompt").unwrap();
    assert_eq!(
        messages(fixture.command("GET soul", vec![]).await.unwrap())[0]["content"],
        "updated prompt"
    );
    assert_eq!(fixture.imports.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn inference_converts_legacy_message_shapes_and_preserves_unknown_stop_reason_history() {
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let input = json!([user("question"), {"type": "assistant", "content": null, "tool_calls": [{"id": "call", "tool_name": "bash", "arguments": "{\"x\":1}"}]}, {"type": "tool_call", "tool_call_id": "call", "tool_name": "bash", "content": "done", "failed": false}, {"type": "assistant", "content": "cancelled fragment", "stop_reason": "cancelled"}]);
    let result = fixture
        .command("EFFECT inference ?", vec![input])
        .await
        .unwrap();
    let MclValue::Inference {
        message,
        stop_reason,
    } = result
    else {
        panic!("inference");
    };
    assert_eq!(message["content"], "response");
    assert_eq!(stop_reason, "completed");
    let records = fixture.inference.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].messages.len(), 4);
    assert!(
        matches!(&records[0].messages[3], InferenceMessage::Assistant { content, .. } if content.as_deref() == Some("cancelled fragment"))
    );
    assert!(
        matches!(&records[0].messages[1], InferenceMessage::Assistant { tool_calls, .. } if tool_calls[0].name == "bash" && tool_calls[0].arguments == json!({"x": 1}))
    );
    assert!(
        matches!(&records[0].messages[2], InferenceMessage::Tool { call_id, .. } if call_id == "call")
    );
    drop(records);
    fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Idle
    );
}

#[tokio::test]
async fn real_http_unknown_tool_is_preserved_then_fails_current_tool_admission() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        let mut byte = [0];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
        }
        let length = String::from_utf8(header)
            .unwrap()
            .to_ascii_lowercase()
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "bash");
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        let trace = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"unknown-call","function":{"name":"unadvertised_tool","arguments":"{\"value\":\"kept\"}"}}]},"finish_reason":"tool_calls"}]})
        );
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{trace}", trace.len()).as_bytes()).await.unwrap();
    });
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.auto_inference.store(false, Ordering::Release);
    let config = fixture.root.path().join("http-models.toml");
    fs::write(&config, format!("[[models]]\nid='test/model'\nmodel='upstream'\nprovider='openai'\napi_type='chat_completions'\nbase_url='{url}'\napi_key='fixture'\ncontext_window='128k'\n")).unwrap();
    fixture
        .app
        .add_plugin(inference_plugin::InferencePlugin::open(&config).unwrap());
    let _ready = fixture.vm("http-driver");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let MclValue::Inference {
        message,
        stop_reason,
    } = fixture
        .command("EFFECT inference ?", vec![json!([user("call the tool")])])
        .await
        .unwrap()
    else {
        panic!("inference must accept a structurally valid unknown tool")
    };
    assert_eq!(stop_reason, "tool_calls");
    assert_eq!(message["type"], "assistant");
    assert_eq!(message["tool_calls"][0]["id"], "unknown-call");
    assert_eq!(message["tool_calls"][0]["tool_name"], "unadvertised_tool");
    assert_eq!(
        serde_json::from_str::<Value>(message["tool_calls"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        json!({"value":"kept"})
    );
    let results = messages(
        fixture
            .command("EFFECT tool_call ?", vec![json!([message])])
            .await
            .unwrap(),
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["tool_call_id"], "unknown-call");
    assert_eq!(results[0]["tool_name"], "unadvertised_tool");
    assert_eq!(results[0]["failed"], true);
    assert_eq!(results[0]["content"], "tool is not imported");
    assert!(fixture.tools.lock().unwrap().is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn no_turn_returns_normal_completed_results_without_submitting_domains() {
    let mut fixture = Fixture::new();
    let result = fixture
        .command("EFFECT inference ?", vec![json!([user("question")])])
        .await
        .unwrap();
    assert!(
        matches!(result, MclValue::Inference { stop_reason, .. } if stop_reason == "completed")
    );
    let calls = json!([{"type": "assistant", "content": null, "tool_calls": [{"id": "call", "tool_name": "bash", "arguments": "{}"}]}]);
    let result = messages(
        fixture
            .command("EFFECT tool_call ?", vec![calls])
            .await
            .unwrap(),
    );
    assert_eq!(result[0]["content"], "Tool call aborted.");
    assert_eq!(result[0]["tool_call_id"], "call");
    assert!(fixture.inference.lock().unwrap().is_empty());
    assert!(fixture.tools.lock().unwrap().is_empty());
}

#[tokio::test]
async fn tool_results_preserve_order_and_return_without_waiting_for_tool_vm_end() {
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let expected_turn = fixture
        .app
        .world()
        .get_component::<Agent>(fixture.owner)
        .unwrap()
        .turn
        .clone()
        .unwrap();
    let calls = json!([{"type": "assistant", "content": null, "tool_calls": [{"id": "first", "tool_name": "bash", "arguments": "{\"n\":1}"}, {"id": "hidden", "tool_name": "private", "arguments": "{}"}, {"id": "last", "tool_name": "bash", "arguments": "{\"n\":2}"}]}]);
    let results = messages(
        fixture
            .command("EFFECT tool_call ?", vec![calls])
            .await
            .unwrap(),
    );
    assert_eq!(
        results
            .iter()
            .map(|value| value["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "hidden", "last"]
    );
    assert_eq!(results[1]["content"], "tool is not imported");
    assert_eq!(results[1]["failed"], true);
    assert!(
        fixture
            .tools
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.turn == expected_turn)
    );
    let tasks = fixture
        .app
        .world()
        .get_component::<ToolTasks>(fixture.owner)
        .unwrap();
    assert_eq!(tasks.list().len(), 2);
    assert!(
        tasks
            .list()
            .iter()
            .all(|(_, task)| !task.is_cleanup_complete())
    );
    fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
    assert!(
        fixture
            .app
            .world()
            .get_component::<ToolTasks>(fixture.owner)
            .unwrap()
            .list()
            .iter()
            .all(|(_, task)| !task.cancel_token().is_cancelled())
    );
}

#[tokio::test]
async fn batch_collects_forced_abort_and_returned_background_replies_without_losing_bindings() {
    // This exercises MCL event transport and real task reply gates; tool execution is a fixture.
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    fixture
        .tool_modes
        .lock()
        .unwrap()
        .insert("pending".into(), 5);
    let mut receiver = fixture.enqueue(
        "EFFECT tool_call ?",
        vec![json!([{"type":"assistant","tool_calls":[
            {"id":"pending","tool_name":"bash","arguments":"{}"},
            {"id":"background","tool_name":"bash","arguments":"{\"background\":true}"}
        ]}])],
    );
    let recorded = fixture.tools.clone();
    tick_until(&mut fixture.app, || recorded.lock().unwrap().len() == 2).await;
    assert!(receiver.try_recv().is_err());
    let requests = recorded.lock().unwrap().clone();
    let tasks = fixture
        .app
        .world()
        .get_component::<ToolTasks>(fixture.owner)
        .unwrap()
        .clone();
    let pending = tasks.get(&requests[0].id).unwrap();
    let background = tasks.get(&requests[1].id).unwrap();
    assert!(!pending.has_returned());
    assert!(background.has_returned());
    let turn = fixture
        .app
        .world()
        .get_component::<Agent>(fixture.owner)
        .unwrap()
        .turn
        .clone()
        .unwrap();
    fixture
        .app
        .world_mut()
        .agent_abort_turn(fixture.owner, &turn)
        .unwrap();
    assert_eq!(tasks.abort_unreturned(), 1);
    let results = messages(drive(&mut fixture.app, &mut receiver).await.unwrap());
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_call_id"], "pending");
    assert_eq!(results[0]["content"], "Tool call aborted.");
    assert_eq!(results[0]["failed"], true);
    assert_eq!(results[1]["tool_call_id"], "background");
    assert_eq!(results[1]["failed"], false);
    assert!(pending.cancel_token().is_cancelled());
    assert!(!background.cancel_token().is_cancelled());
    fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Idle
    );
    assert!(!pending.is_cleanup_complete());
    assert!(tasks.get(&requests[1].id).is_ok());
    assert!(
        fixture
            .app
            .world()
            .get_component::<MclQueue>(fixture.owner)
            .unwrap()
            .pending
            .is_none()
    );
}

#[tokio::test]
async fn fifo_waits_for_domain_reply_while_other_agents_keep_progressing() {
    let mut fixture = Fixture::new();
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    fixture.auto_inference.store(false, Ordering::Release);
    let mut first = fixture.enqueue("EFFECT inference ?", vec![json!([user("question")])]);
    let mut queued = fixture.enqueue("CONFIG SET ? ?", vec![json!("ordered"), json!(true)]);
    let recorded = fixture.inference.clone();
    tick_until(&mut fixture.app, || !recorded.lock().unwrap().is_empty()).await;
    assert!(queued.try_recv().is_err());
    let workspace = fixture
        .app
        .world()
        .get_component::<Agent>(fixture.owner)
        .unwrap()
        .workspace;
    let other = fixture.app.world_mut().spawn();
    fixture.app.world_mut().insert_component(
        other,
        Agent {
            name: "other".into(),
            workspace,
            state: AgentState::Idle,
            turn: None,
        },
    );
    let shared_storage = fixture
        .app
        .world()
        .get_component::<AgentStorage>(fixture.owner)
        .unwrap()
        .clone();
    fixture
        .app
        .world_mut()
        .insert_component(other, shared_storage);
    let mut other_reply = fixture.enqueue_as(
        "other-read".into(),
        fixture.client,
        None,
        other,
        "CONFIG GET",
        vec![],
    );
    assert!(matches!(
        drive(&mut fixture.app, &mut other_reply).await.unwrap(),
        MclValue::Config(_)
    ));
    let request = fixture.inference.lock().unwrap()[0].clone();
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(response()))
        .unwrap();
    drive(&mut fixture.app, &mut first).await.unwrap();
    drive(&mut fixture.app, &mut queued).await.unwrap();
}

#[tokio::test]
async fn closing_queue_cancels_domain_and_waits_for_reply_before_drain() {
    let mut fixture = Fixture::new();
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    fixture.auto_inference.store(false, Ordering::Release);
    let mut first = fixture.enqueue("EFFECT inference ?", vec![json!([user("question")])]);
    let mut queued = fixture.enqueue("CONFIG SET ? ?", vec![json!("must_not_exist"), json!(true)]);
    let recorded = fixture.inference.clone();
    tick_until(&mut fixture.app, || !recorded.lock().unwrap().is_empty()).await;
    let drain = fixture.app.world().mcl_drain(fixture.owner).unwrap();
    fixture.app.world_mut().mcl_close(fixture.owner).unwrap();
    let request = fixture.inference.lock().unwrap()[0].clone();
    assert!(request.cancel.is_cancelled());
    let mut waiting = Box::pin(drain.wait());
    assert!(
        tokio::time::timeout(Duration::from_millis(5), &mut waiting)
            .await
            .is_err()
    );
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(InferenceResponse::empty_completed()))
        .unwrap();
    drive(&mut fixture.app, &mut first).await.unwrap();
    assert_eq!(
        drive(&mut fixture.app, &mut queued).await.error_kind(),
        MclErrorKind::Cancelled
    );
    waiting.await.unwrap();
    let storage = fixture
        .app
        .world()
        .get_component::<AgentStorage>(fixture.owner)
        .unwrap();
    let StorageValue::Config(config) =
        execute_storage(storage, StorageOperation::ReadConfig).unwrap()
    else {
        panic!("config");
    };
    assert!(!config.contains_key("must_not_exist"));
    assert_eq!(
        fixture.command("CONFIG GET", vec![]).await.error_kind(),
        MclErrorKind::Closed
    );
}

#[tokio::test]
async fn ready_requires_current_self_vm_and_wake_uses_current_slot() {
    let mut fixture = Fixture::new();
    let ready = fixture.vm("current");
    fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.owner)
        .unwrap()
        .state = AgentState::Starting;
    assert_eq!(
        fixture.command("EFFECT ready", vec![]).await.error_kind(),
        MclErrorKind::SourceInvalid
    );
    let mut stale = fixture.enqueue_as(
        "stale".into(),
        fixture.owner,
        Some("old".into()),
        fixture.owner,
        "EFFECT ready",
        vec![],
    );
    assert_eq!(
        drive(&mut fixture.app, &mut stale).await.error_kind(),
        MclErrorKind::SourceInvalid
    );
    let mut valid = fixture.enqueue_as(
        "ready".into(),
        fixture.owner,
        Some("current".into()),
        fixture.owner,
        "EFFECT ready",
        vec![],
    );
    drive(&mut fixture.app, &mut valid).await.unwrap();
    ready.await.unwrap().unwrap();
    fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.owner)
        .unwrap()
        .state = AgentState::Idle;
    fixture.command("EFFECT wake", vec![]).await.unwrap();
    let slot = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(fixture.owner)
        .unwrap()
        .wake
        .clone();
    DriverWakeSlot::wait(slot).await.unwrap();
}

#[tokio::test]
async fn finish_turn_reports_running_inference_and_keeps_abort_control() {
    let mut fixture = Fixture::new();
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let turn = fixture
        .app
        .world()
        .get_component::<Agent>(fixture.owner)
        .unwrap()
        .turn
        .clone()
        .unwrap();
    let (release, waiting) = oneshot::channel();
    let cancel = CancellationToken::new();
    fixture
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(fixture.owner)
        .unwrap()
        .current = Some(attached_inference(
        InferenceHandle {
            request_id: "active".into(),
            turn: turn.clone(),
            cancel: cancel.clone(),
            stop_stream: Arc::new(|| {}),
            task: Arc::new(InferenceTaskState::default()),
        },
        tokio::spawn(async move {
            waiting.await.unwrap();
        }),
    ));
    let mut reply = fixture.enqueue("EFFECT finish_turn", vec![]);
    assert_eq!(
        drive(&mut fixture.app, &mut reply).await.error_kind(),
        MclErrorKind::EffectFailed
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Working
    );
    fixture
        .app
        .world()
        .get_component::<ActiveInference>(fixture.owner)
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .cancel();
    assert!(cancel.is_cancelled());
    fixture
        .app
        .world_mut()
        .agent_abort_turn(fixture.owner, &turn)
        .unwrap();
    release.send(()).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        fixture.command("EFFECT finish_turn", vec![]).await.unwrap(),
        MclValue::Unit
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Idle
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<ActiveInference>(fixture.owner)
            .unwrap()
            .current
            .is_none()
    );
}

#[tokio::test]
async fn history_append_records_messages_once_and_keeps_command_audits_separate() {
    let mut fixture = Fixture::new();
    fixture
        .command(
            "EFFECT history_append ?",
            vec![json!([user("one"), user("two")])],
        )
        .await
        .unwrap();
    let records = fixture.history();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.kind == "message")
            .count(),
        2
    );
    assert_eq!(
        records.iter().filter(|record| record.kind == "mcl").count(),
        1
    );
    let payloads = records
        .iter()
        .filter(|record| record.kind == "message")
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    assert_eq!(payloads, [user("one"), user("two")]);
    assert_eq!(
        fixture
            .command(
                "EFFECT history_append ?",
                vec![json!([{"type": "unknown"}])]
            )
            .await
            .error_kind(),
        MclErrorKind::InvalidMessage
    );
    assert_eq!(
        fixture
            .history()
            .iter()
            .filter(|record| record.kind == "message")
            .count(),
        2
    );
}

#[tokio::test]
async fn rejects_duplicate_inflight_ids_and_allows_same_id_after_completion() {
    let mut fixture = Fixture::new();
    let mut first = fixture.enqueue_as(
        "same".into(),
        fixture.client,
        None,
        fixture.owner,
        "CONFIG GET",
        vec![],
    );
    let mut duplicate = fixture.enqueue_as(
        "same".into(),
        fixture.client,
        None,
        fixture.owner,
        "CONFIG GET",
        vec![],
    );
    assert_eq!(
        drive(&mut fixture.app, &mut duplicate).await.error_kind(),
        MclErrorKind::ParseFailed
    );
    drive(&mut fixture.app, &mut first).await.unwrap();
    let mut next = fixture.enqueue_as(
        "same".into(),
        fixture.client,
        None,
        fixture.owner,
        "CONFIG SET ? ?",
        vec![json!("repeat"), json!(true)],
    );
    drive(&mut fixture.app, &mut next).await.unwrap();
}

#[tokio::test]
async fn reusing_a_completed_request_id_creates_distinct_background_task_addresses() {
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    for _ in 0..2 {
        let calls = json!([{"type":"assistant","tool_calls":[{"id":"call","tool_name":"bash","arguments":"{}"}]}]);
        let mut receiver = fixture.enqueue_as(
            "reused".into(),
            fixture.client,
            None,
            fixture.owner,
            "EFFECT tool_call ?",
            vec![calls],
        );
        drive(&mut fixture.app, &mut receiver).await.unwrap();
    }
    let tasks = fixture
        .app
        .world()
        .get_component::<ToolTasks>(fixture.owner)
        .unwrap()
        .list();
    assert_eq!(tasks.len(), 2);
    assert_ne!(tasks[0].0, tasks[1].0);
}

#[tokio::test]
async fn inference_error_and_finish_turn_return_values_before_cleanup_completes() {
    for kind in [
        InferenceErrorKind::RequestFailed,
        InferenceErrorKind::ResponseStatus,
        InferenceErrorKind::Timeout,
        InferenceErrorKind::CleanupFailed,
    ] {
        let mut fixture = Fixture::new();
        fixture.auto_inference.store(false, Ordering::Release);
        fixture.command("EFFECT start_turn", vec![]).await.unwrap();
        let mut reply = fixture.enqueue("EFFECT inference ?", vec![json!([user("hello")])]);
        let requests = fixture.inference.clone();
        tick_until(&mut fixture.app, || !requests.lock().unwrap().is_empty()).await;
        let request = requests.lock().unwrap()[0].clone();
        let (release, waiting) = oneshot::channel();
        let cancel = CancellationToken::new();
        fixture
            .app
            .world_mut()
            .get_component_mut::<ActiveInference>(fixture.owner)
            .unwrap()
            .current = Some(attached_inference(
            InferenceHandle {
                request_id: request.id.clone(),
                turn: request.turn.clone(),
                cancel: cancel.clone(),
                stop_stream: Arc::new(|| {}),
                task: Arc::new(InferenceTaskState::default()),
            },
            tokio::spawn(async move {
                waiting.await.unwrap();
            }),
        ));
        request
            .reply
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(Err(InferenceError::new(kind, "safe failure")))
            .unwrap();
        let MclValue::Error(message) = drive(&mut fixture.app, &mut reply).await.unwrap() else {
            panic!("inference value")
        };
        assert_eq!(
            message,
            json!({"type": "error", "message": format!("EffectFailed: {kind:?}: safe failure"), "request_id": request.id})
        );
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<Agent>(fixture.owner)
                .unwrap()
                .state,
            AgentState::Working
        );
        let mut finish = fixture.enqueue("EFFECT finish_turn", vec![]);
        assert_eq!(
            drive(&mut fixture.app, &mut finish).await.error_kind(),
            MclErrorKind::EffectFailed
        );
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<Agent>(fixture.owner)
                .unwrap()
                .state,
            AgentState::Working
        );
        let active = fixture
            .app
            .world()
            .get_component::<ActiveInference>(fixture.owner)
            .unwrap()
            .current
            .as_ref()
            .unwrap();
        assert!(!active.is_finished());
        active.cancel();
        assert!(cancel.is_cancelled());
        release.send(()).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(
            fixture.command("EFFECT finish_turn", vec![]).await.unwrap(),
            MclValue::Unit
        );
        let agent = fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap();
        assert_eq!(agent.state, AgentState::Idle);
        assert!(agent.turn.is_none());
        assert!(
            fixture
                .app
                .world()
                .get_component::<ActiveInference>(fixture.owner)
                .unwrap()
                .current
                .is_none()
        );
    }
}

#[tokio::test]
async fn inference_error_does_not_finish_a_replacement_turn() {
    let mut fixture = Fixture::new();
    fixture.auto_inference.store(false, Ordering::Release);
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let mut reply = fixture.enqueue("EFFECT inference ?", vec![json!([user("hello")])]);
    let requests = fixture.inference.clone();
    tick_until(&mut fixture.app, || !requests.lock().unwrap().is_empty()).await;
    let request = requests.lock().unwrap()[0].clone();
    fixture
        .app
        .world_mut()
        .agent_abort_turn(fixture.owner, &request.turn)
        .unwrap();
    fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.owner)
        .unwrap()
        .turn = Some("replacement".into());
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Err(InferenceError::new(
            InferenceErrorKind::Timeout,
            "safe failure",
        )))
        .unwrap();
    let MclValue::Error(message) = drive(&mut fixture.app, &mut reply).await.unwrap() else {
        panic!("inference value")
    };
    assert_eq!(
        message,
        json!({"type":"error", "message":"EffectFailed: Timeout: safe failure", "request_id":request.id})
    );
    let agent = fixture
        .app
        .world()
        .get_component::<Agent>(fixture.owner)
        .unwrap();
    assert_eq!(agent.state, AgentState::Working);
    assert_eq!(agent.turn.as_deref(), Some("replacement"));
}

#[tokio::test]
async fn bundled_drivers_persist_inference_errors_and_keep_the_same_vm_for_next_message() {
    for source in [
        include_str!("../../../../resources/agent-drivers/local/coder/latest/base.lua"),
        include_str!("../../../../resources/agent-drivers/local/reviewer/latest/base.lua"),
    ] {
        let mut fixture = Fixture::new();
        fixture.auto_inference.store(false, Ordering::Release);
        fixture.app.add_plugin(LuaRuntimePlugin::new());
        let workspace = fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .workspace;
        fixture.app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace {
                name: "bundled-driver".into(),
                definition_path: fixture.root.path().join("workspace.yaml"),
                project_root: fixture.root.path().to_owned(),
                state: workspace_core::WorkspaceState::Running,
                agents: Default::default(),
            },
        );
        let executor = workspace_core::spawn_workspace_executor().unwrap();
        let close = executor.close.clone();
        fixture
            .app
            .world_mut()
            .insert_component(workspace, executor);
        // Use the bundled Driver unchanged; fake only resource imports and inference.
        let mut allowed = Vec::new();
        for line in source
            .lines()
            .map(str::trim_start)
            .filter(|line| line.starts_with("command(\"IMPORT "))
        {
            let address = line.split_whitespace().nth(2).unwrap();
            allowed.push(address.to_owned());
            let (kind, rest) = address.split_once(':').unwrap();
            let (name, version) = rest.rsplit_once(':').unwrap();
            let root = match kind {
                "prompt" => "prompts",
                "tool" => "tools",
                "sandbox" => "sandboxes",
                _ => unreachable!(),
            };
            let directory = fixture.root.path().join(root).join(name).join(version);
            fs::create_dir_all(&directory).unwrap();
            match kind {
                "prompt" => {
                    fs::write(directory.join("prompt.toml"), "role='system'\n").unwrap();
                    fs::write(directory.join("content.md"), "test instructions").unwrap();
                }
                "tool" => {
                    fs::write(
                        directory.join("tool.toml"),
                        "schema_version=1\ndescription='test tool'\n",
                    )
                    .unwrap();
                    fs::write(directory.join("input.schema.json"), "{}").unwrap();
                    fs::write(directory.join("main.lua"), "function configure(arguments, context) return {timeout_ms=30000} end\nfunction execute() margatroid.task:return_result('test') end").unwrap();
                }
                "sandbox" => fs::write(directory.join("policy.json"), "{}").unwrap(),
                _ => unreachable!(),
            }
        }
        fixture
            .app
            .world_mut()
            .get_component_mut::<MclQueue>(fixture.owner)
            .unwrap()
            .allowed_imports = allowed;
        for key in ["sandbox"] {
            fixture
                .command("CONFIG SET ? ?", vec![json!(key), json!([])])
                .await
                .unwrap();
        }
        fixture
            .app
            .world_mut()
            .get_component_mut::<Agent>(fixture.owner)
            .unwrap()
            .state = AgentState::Starting;
        let environment = LuaEnvironment {
            globals: std::collections::BTreeMap::from([("agent_info".into(), LuaBinding::Value(LuaValue::from_json(json!({
                "entity": {"index": fixture.owner.index(), "generation": fixture.owner.generation()},
                "model": {"context_window_tokens": 4096}
            })).unwrap()))]),
            modules: Default::default(),
        };
        let (sender, mut started) = oneshot::channel();
        fixture.app.world().send_event(StartDriverLuaVm {
            owner: fixture.owner,
            workspace,
            program: LuaProgram {
                source: source.into(),
                origin: "bundled-driver".into(),
                libraries: LuaStandardLibraries::Minimal,
            },
            environment,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        let instance = drive(&mut fixture.app, &mut started).await.unwrap();
        fixture
            .app
            .world_mut()
            .get_component_mut::<Agent>(fixture.owner)
            .unwrap()
            .state = AgentState::Idle;
        for (index, kind) in [
            Some(InferenceErrorKind::RequestFailed),
            Some(InferenceErrorKind::ResponseStatus),
            Some(InferenceErrorKind::Timeout),
            Some(InferenceErrorKind::CleanupFailed),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            fixture
                .command(
                    "INJECT ? TO entry.pending[-1]",
                    vec![json!([user(&format!("message-{index}"))])],
                )
                .await
                .unwrap();
            fixture.command("EFFECT wake", vec![]).await.unwrap();
            let recorded = fixture.inference.clone();
            tick_until(&mut fixture.app, || recorded.lock().unwrap().len() > index).await;
            let request = recorded.lock().unwrap()[index].clone();
            // The reply can precede task exit: finish_turn must retain this handle
            // and Working state until the inference supervisor really exits.
            let (release, waiting) = oneshot::channel();
            let cancel = CancellationToken::new();
            fixture
                .app
                .world_mut()
                .get_component_mut::<ActiveInference>(fixture.owner)
                .unwrap()
                .current = Some(attached_inference(
                InferenceHandle {
                    request_id: request.id.clone(),
                    turn: request.turn.clone(),
                    cancel: cancel.clone(),
                    stop_stream: Arc::new(|| {}),
                    task: Arc::new(InferenceTaskState::default()),
                },
                tokio::spawn(async move {
                    waiting.await.unwrap();
                }),
            ));
            let failure = kind.map(|kind| InferenceError::new(kind, "safe inference failure"));
            request
                .reply
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(match &failure {
                    Some(failure) => Err(failure.clone()),
                    None => Ok(response()),
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                fixture.app.tick();
                let persisted = fixture.history().iter().any(|record| {
                    record.kind == "message" && match &failure {
                        Some(failure) => record.payload == json!({"type":"error", "message":format!("EffectFailed: {failure}"), "request_id":request.id}),
                        None => record.payload["type"] == "assistant" && record.payload["request_id"] == request.id && record.payload["content"] == "response",
                    }
                });
                if persisted {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Driver did not persist inference result {kind:?} and settle its turn"
                );
                tokio::task::yield_now().await;
            }
            // Give the Driver time to execute its real finish_turn command.
            for _ in 0..30 {
                fixture.app.tick();
                tokio::task::yield_now().await;
            }
            let agent = fixture
                .app
                .world()
                .get_component::<Agent>(fixture.owner)
                .unwrap();
            assert_eq!(agent.state, AgentState::Working);
            assert_eq!(agent.turn.as_deref(), Some(request.turn.as_str()));
            let active = fixture
                .app
                .world()
                .get_component::<ActiveInference>(fixture.owner)
                .unwrap()
                .current
                .as_ref()
                .unwrap();
            assert_eq!(active.request_id, request.id);
            assert!(!active.is_finished());
            active.cancel();
            assert!(cancel.is_cancelled());
            release.send(()).unwrap();
            tokio::task::yield_now().await;
            fixture.command("EFFECT wake", vec![]).await.unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                fixture.app.tick();
                let agent = fixture
                    .app
                    .world()
                    .get_component::<Agent>(fixture.owner)
                    .unwrap();
                if agent.state == AgentState::Idle {
                    assert!(agent.turn.is_none());
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Driver did not finish {kind:?} after inference task exited"
                );
                tokio::task::yield_now().await;
            }
            assert!(
                fixture
                    .app
                    .world()
                    .get_component::<ActiveInference>(fixture.owner)
                    .unwrap()
                    .current
                    .is_none()
            );
            assert_eq!(
                fixture
                    .history()
                    .iter()
                    .filter(|record| record.kind == "message"
                        && record.payload["request_id"] == request.id)
                    .count(),
                1
            );
            let vm = fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .unwrap();
            assert_eq!(vm.instance, instance);
            assert!(!vm.cancel.is_cancelled());
        }
        let (sender, mut stopped) = oneshot::channel();
        fixture.app.world().send_event(StopDriverLuaVm {
            instance,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        drive(&mut fixture.app, &mut stopped).await.unwrap();
        close.close_and_join().await.unwrap();
    }
}

#[tokio::test]
async fn callers_must_supply_their_own_address_before_any_mcl_side_effect() {
    let mut fixture = Fixture::new();
    for (index, source_address) in ["", "agent:local/test:latest", "entity:2:0"]
        .into_iter()
        .enumerate()
    {
        let (sender, mut receiver) = oneshot::channel();
        fixture.app.world().send_event(MclCommand {
            id: format!("bad-source-{index}"),
            source: fixture.client,
            source_address: source_address.into(),
            source_instance: None,
            target: fixture.owner,
            instruction: MclInstruction {
                text: "CONFIG SET ? ?".into(),
                arguments: vec![json!("unexpected"), json!(true)],
            },
            cancel: CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        assert_eq!(
            drive(&mut fixture.app, &mut receiver).await.error_kind(),
            MclErrorKind::SourceInvalid
        );
    }
    assert!(fixture.history().is_empty());
    let MclValue::Config(config) = fixture.command("CONFIG GET", vec![]).await.unwrap() else {
        panic!("config")
    };
    assert!(!config.contains_key("unexpected"));
}

#[tokio::test]
async fn one_executor_records_mcl_caller_addresses_and_message_types_with_content_and_payload() {
    let mut fixture = Fixture::new();
    for (source, address, text) in [
        (
            fixture.client,
            "client:local/frontend:latest",
            "from client",
        ),
        (fixture.owner, "agent:local/test:latest", "from driver"),
    ] {
        let payload = json!({"type":"user", "content":text});
        let mut reply = fixture.enqueue_as(
            format!("append-{text}"),
            source,
            None,
            fixture.owner,
            "EFFECT history_append ?",
            vec![json!([payload.clone()])],
        );
        drive(&mut fixture.app, &mut reply).await.unwrap();
        let history = fixture.history();
        let record = history
            .iter()
            .find(|row| row.kind == "message" && row.content == text)
            .unwrap();
        assert!(record.source.is_none());
        assert_eq!(record.payload, payload);
        let audit = history
            .iter()
            .find(|row| row.kind == "mcl" && row.payload["id"] == format!("append-{text}"))
            .unwrap();
        assert_eq!(audit.source.as_deref(), Some(address));
        assert_eq!(audit.content, "EFFECT history_append ?");
        assert_eq!(audit.payload["text"], audit.content);
    }
}

#[tokio::test]
async fn all_tool_errors_return_failed_results_and_leave_turn_completion_to_driver() {
    for kind in [
        tool_core::ToolErrorKind::InvalidRequest,
        tool_core::ToolErrorKind::AgentMissing,
        tool_core::ToolErrorKind::TaskMissing,
        tool_core::ToolErrorKind::DuplicateTask,
        tool_core::ToolErrorKind::AliasMissing,
        tool_core::ToolErrorKind::InvalidArguments,
        tool_core::ToolErrorKind::InvalidDefinition,
        tool_core::ToolErrorKind::ReadFailed,
        tool_core::ToolErrorKind::Closed,
        tool_core::ToolErrorKind::SubmitFailed,
        tool_core::ToolErrorKind::SandboxUnavailable,
        tool_core::ToolErrorKind::RunnerStartFailed,
        tool_core::ToolErrorKind::RunnerProtocolFailed,
        tool_core::ToolErrorKind::ExecutionFailed,
        tool_core::ToolErrorKind::OutputTooLarge,
        tool_core::ToolErrorKind::Timeout,
        tool_core::ToolErrorKind::Cancelled,
        tool_core::ToolErrorKind::TaskPanicked,
        tool_core::ToolErrorKind::CleanupFailed,
        tool_core::ToolErrorKind::AlreadyReturned,
        tool_core::ToolErrorKind::MissingReturn,
        tool_core::ToolErrorKind::InputClosed,
        tool_core::ToolErrorKind::InputFull,
    ] {
        let mut fixture = Fixture::new();
        let _ready = fixture.vm("same-vm");
        fixture.add_tool("bash_exec");
        fixture.command("EFFECT start_turn", vec![]).await.unwrap();
        *fixture.tool_error.lock().unwrap() = Some(tool_core::ToolError::new(
            kind,
            "shell failed after cleanup",
        ));
        let calls = json!([{"type":"assistant","tool_calls":[{"id":"sleep-call","tool_name":"bash_exec","arguments":"{\"cmd\":\"sleep 11\"}"}]}]);
        let MclValue::Array(MessageArray::Message(results)) = fixture
            .command("EFFECT tool_call ?", vec![calls])
            .await
            .unwrap()
        else {
            panic!("tool result")
        };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["failed"], true);
        assert_eq!(results[0]["tool_call_id"], "sleep-call");
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap()
                .contains("shell failed after cleanup")
        );
        fixture
            .command("EFFECT history_append ?", vec![json!(results)])
            .await
            .unwrap();
        fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<Agent>(fixture.owner)
                .unwrap()
                .state,
            AgentState::Idle
        );
        fixture.command("EFFECT start_turn", vec![]).await.unwrap();
        assert!(matches!(
            fixture.command("CONFIG GET", vec![]).await.unwrap(),
            MclValue::Config(_)
        ));
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .unwrap()
                .instance
                .start_id,
            "same-vm"
        );
        assert!(
            !fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .unwrap()
                .cancel
                .is_cancelled()
        );
    }
}

#[tokio::test]
async fn tool_cleanup_failures_return_failed_messages_and_preserve_the_turn() {
    for kind in [
        tool_core::ToolErrorKind::CleanupFailed,
        tool_core::ToolErrorKind::TaskPanicked,
    ] {
        let mut fixture = Fixture::new();
        let _ready = fixture.vm("same-vm");
        fixture.add_tool("bash_exec");
        fixture.command("EFFECT start_turn", vec![]).await.unwrap();
        *fixture.tool_error.lock().unwrap() =
            Some(tool_core::ToolError::new(kind, "cleanup not proven"));
        let calls = json!([{"type":"assistant","tool_calls":[{"id":"call","tool_name":"bash_exec","arguments":"{}"}]}]);
        let results = messages(
            fixture
                .command("EFFECT tool_call ?", vec![calls])
                .await
                .unwrap(),
        );
        assert_eq!(results[0]["type"], "tool_call");
        assert_eq!(results[0]["tool_call_id"], "call");
        assert_eq!(results[0]["tool_name"], "bash_exec");
        assert_eq!(results[0]["failed"], true);
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap()
                .contains("cleanup not proven")
        );
        assert_eq!(
            fixture
                .app
                .world()
                .get_component::<Agent>(fixture.owner)
                .unwrap()
                .state,
            AgentState::Working
        );
        assert!(
            fixture
                .history()
                .iter()
                .all(|record| record.kind != "message")
        );
        fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
        fixture.command("EFFECT start_turn", vec![]).await.unwrap();
        assert!(matches!(
            fixture.command("CONFIG GET", vec![]).await.unwrap(),
            MclValue::Config(_)
        ));
    }
}

#[tokio::test]
async fn mixed_tool_batch_keeps_all_calls_after_backend_and_association_failures() {
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    *fixture.tool_modes.lock().unwrap() = std::collections::BTreeMap::from([
        ("wrong-id".into(), 1),
        ("wrong-alias".into(), 2),
        ("closed".into(), 3),
        ("cleanup".into(), 4),
    ]);
    let ids = [
        "first",
        "wrong-id",
        "wrong-alias",
        "closed",
        "cleanup",
        "last",
    ];
    let calls = ids
        .iter()
        .map(|id| json!({"id":id,"tool_name":"bash","arguments":"{}"}))
        .collect::<Vec<_>>();
    let results = messages(
        fixture
            .command(
                "EFFECT tool_call ?",
                vec![json!([{"type":"assistant","tool_calls":calls}])],
            )
            .await
            .unwrap(),
    );
    assert_eq!(results.len(), ids.len());
    for (index, (message, id)) in results.iter().zip(ids).enumerate() {
        assert_eq!(message["type"], "tool_call");
        assert_eq!(message["tool_call_id"], id);
        assert_eq!(message["tool_name"], "bash");
        assert_eq!(message["failed"], index != 0 && index != 5);
    }
    assert!(
        results[1]["content"]
            .as_str()
            .unwrap()
            .contains("association")
    );
    assert!(
        results[2]["content"]
            .as_str()
            .unwrap()
            .contains("association")
    );
    assert!(
        results[3]["content"]
            .as_str()
            .unwrap()
            .contains("reply was lost")
    );
    assert!(
        results[4]["content"]
            .as_str()
            .unwrap()
            .contains("cleanup not proven")
    );
    assert_eq!(fixture.tools.lock().unwrap().len(), ids.len());
    assert!(
        fixture
            .history()
            .iter()
            .all(|record| record.kind != "message")
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Working
    );
    fixture.command("EFFECT finish_turn", vec![]).await.unwrap();
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    assert!(matches!(
        fixture.command("CONFIG GET", vec![]).await.unwrap(),
        MclValue::Config(_)
    ));
}

#[tokio::test]
async fn parse_and_effect_preconditions_return_errors_without_fabricated_tool_ids() {
    let mut fixture = Fixture::new();
    for (text, arguments, kind) in [
        ("NOT AN INSTRUCTION", vec![], MclErrorKind::ParseFailed),
        (
            "EFFECT tool_call ?",
            vec![
                json!([{"type":"assistant","tool_calls":[{"tool_name":"bash","arguments":"{}"}]}]),
            ],
            MclErrorKind::InvalidMessage,
        ),
        (
            "EFFECT tool_call (missing)",
            vec![],
            MclErrorKind::AliasMissing,
        ),
        (
            "EFFECT tool_call ?",
            vec![json!("not a message array")],
            MclErrorKind::InvalidMessage,
        ),
    ] {
        assert_eq!(fixture.command(text, arguments).await.error_kind(), kind);
    }
    assert!(fixture.tools.lock().unwrap().is_empty());
    assert!(
        fixture
            .history()
            .iter()
            .all(|record| record.kind != "message")
    );
    let history = fixture.history();
    assert!(
        history
            .iter()
            .filter(|record| record.kind == "mcl")
            .all(|record| record.payload["succeeded"] == false)
    );
    assert!(matches!(
        fixture.command("CONFIG GET", vec![]).await.unwrap(),
        MclValue::Config(_)
    ));
}

#[tokio::test]
async fn audit_persistence_failure_returns_error_and_discloses_executed_side_effects() {
    let mut fixture = Fixture::new();
    let storage = fixture
        .app
        .world_mut()
        .remove_component::<AgentStorage>(fixture.owner)
        .unwrap();
    let MclValue::Error(message) = fixture.command("EFFECT start_turn", vec![]).await.unwrap()
    else {
        panic!("audit failure must return Error")
    };
    assert_eq!(message["type"], "error");
    let detail = message["message"].as_str().unwrap();
    assert!(detail.contains("audit persistence failed after execution"));
    assert!(detail.contains("side effects may already have occurred"));
    assert!(detail.contains("execution outcome: succeeded"));
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(fixture.owner)
            .unwrap()
            .state,
        AgentState::Working
    );
    fixture
        .app
        .world_mut()
        .insert_component(fixture.owner, storage);
    assert_eq!(
        fixture.command("EFFECT finish_turn", vec![]).await.unwrap(),
        MclValue::Unit
    );
    assert!(
        fixture
            .history()
            .iter()
            .all(|record| record.kind != "message")
    );
}

#[tokio::test]
async fn malformed_identifiable_tool_arguments_fail_locally_and_other_calls_run() {
    let mut fixture = Fixture::new();
    fixture.add_tool("bash");
    fixture.command("EFFECT start_turn", vec![]).await.unwrap();
    let input = json!([{"type":"assistant","tool_calls":[
        {"id":"first","tool_name":"bash","arguments":"{}"},
        {"id":"bad-json","tool_name":"bash","arguments":"{"},
        {"id":"missing-args","tool_name":"bash"},
        {"id":"last","tool_name":"bash","arguments":"{}"}
    ]}]);
    let results = messages(
        fixture
            .command("EFFECT tool_call ?", vec![input])
            .await
            .unwrap(),
    );
    assert_eq!(results.len(), 4);
    for (index, id) in ["first", "bad-json", "missing-args", "last"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(results[index]["tool_call_id"], id);
        assert_eq!(results[index]["tool_name"], "bash");
        assert_eq!(results[index]["failed"], index == 1 || index == 2);
    }
    assert_eq!(fixture.tools.lock().unwrap().len(), 2);
    assert!(
        fixture
            .history()
            .iter()
            .all(|record| record.kind != "message")
    );
    let invalid_batch = json!([{"type":"assistant","tool_calls":[
        {"id":"duplicate","tool_name":"bash","arguments":"{}"},
        {"id":"duplicate","tool_name":"bash","arguments":"{}"}
    ]}]);
    assert_eq!(
        fixture
            .command("EFFECT tool_call ?", vec![invalid_batch])
            .await
            .error_kind(),
        MclErrorKind::InvalidMessage
    );
    assert_eq!(
        fixture.tools.lock().unwrap().len(),
        2,
        "identity rejection must precede dispatch"
    );
}

fn attached_inference(
    handle: InferenceHandle,
    task: tokio::task::JoinHandle<()>,
) -> InferenceHandle {
    handle.attach_task(task).unwrap();
    handle
}
