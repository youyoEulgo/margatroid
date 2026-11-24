use address::Address;
use agent_core::{Agent, WorldAgentExt};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use axum_plugin::{AxumHandle, AxumPlugin};
use client_core::{Client, ConnectionId, WorldClientExt};
use client_plugin::ClientPlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use config_core::{LogLevel, MargatroidConfig};
use core_plugin::{App, Entity, World};
use dto_plugin::DtoPlugin;
use futures_util::{SinkExt, StreamExt};
use inference_core::{
    ActiveInference, InferenceHandle, InferenceRequest, InferenceResponse, InferenceStopReason,
};
use mcl_core::MclQueue;
use mcl_plugin::MclPlugin;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage_core::{AgentStorage, HistoryRecord, StorageOperation, execute_storage, open_storage};
use storage_plugin::StoragePlugin;
use tokio::sync::oneshot;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tool_core::{ToolExecutionLimits, ToolTaskHandle, ToolTasks};
use workspace_core::{
    CreateWorkspace, StartWorkspaceAgent, WorkspaceDatabase, WorkspaceMetadata, WorkspaceState,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Harness {
    app: App,
    runtime: tokio::runtime::Runtime,
    server: SocketAddr,
    root: tempfile::TempDir,
    workspace: Entity,
    agent: Entity,
    workspace_creates: Arc<Mutex<Vec<CreateWorkspace>>>,
    agent_starts:
        Arc<Mutex<Vec<StartWorkspaceAgent<tool_core::ToolExecuteResponse, tool_core::ToolError>>>>,
    inference: Arc<Mutex<Vec<InferenceRequest>>>,
}

impl Harness {
    fn new() -> Self {
        Self::with_counters(None)
    }
    fn with_counters(
        counters: Option<(
            Arc<std::sync::atomic::AtomicUsize>,
            Arc<std::sync::atomic::AtomicUsize>,
        )>,
    ) -> Self {
        Self::with_counters_and_ingress(counters, config_core::IngressPolicy::Localhost)
    }
    fn with_counters_and_ingress(
        counters: Option<(
            Arc<std::sync::atomic::AtomicUsize>,
            Arc<std::sync::atomic::AtomicUsize>,
        )>,
        policy: config_core::IngressPolicy,
    ) -> Self {
        Self::build(counters, policy, false)
    }
    fn with_real_workspace() -> Self {
        Self::build(None, config_core::IngressPolicy::Localhost, true)
    }
    fn build(
        counters: Option<(
            Arc<std::sync::atomic::AtomicUsize>,
            Arc<std::sync::atomic::AtomicUsize>,
        )>,
        policy: config_core::IngressPolicy,
        real_workspace: bool,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let initial_root = root.path().join("agent-drivers");
        for name in ["new", "refreshed"] {
            let source = initial_root.join("margatroid").join(name).join("dev");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join("base.lua"), "").unwrap();
        }

        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(AxumPlugin::bind("127.0.0.1:0".parse().unwrap()))
            .add_plugin(ClientPlugin::new())
            .add_plugin(StoragePlugin::new())
            .add_plugin(MclPlugin::new());
        app.world_mut().insert_resource(
            MargatroidConfig::new("127.0.0.1:0".parse().unwrap(), LogLevel::Info, None)
                .unwrap()
                .with_ingress(policy)
                .unwrap(),
        );
        let definitions_root = root.path().join("workspaces");
        let managed = definitions_root.join("demo");
        for kind in ["normal", "temporary", "archived"] {
            std::fs::create_dir_all(managed.join("agents").join(kind)).unwrap();
        }
        let database = WorkspaceDatabase::create(
            managed.join("workspace.sqlite"),
            WorkspaceMetadata {
                address: "/demo".into(),
                alias: "demo".into(),
                project_root: root.path().into(),
            },
        )
        .unwrap();
        app.world_mut()
            .insert_resource(workspace_core::WorkspaceConfig {
                root: definitions_root,
            });
        let bindings = dto_plugin::sync_bindings(
            app.world()
                .get_resource::<client_core::Clients>()
                .unwrap()
                .clone(),
        );
        app.world_mut().insert_resource(bindings.workspace_binding);
        app.world_mut().insert_resource(bindings.workspace);
        app.world_mut().insert_resource(bindings.agent);
        app.world_mut().insert_resource(bindings.models);
        app.world_mut().insert_resource(bindings.sandboxes);
        app.world_mut()
            .insert_resource(dto_plugin::TaskSyncInstaller(bindings.tasks));
        let workspace = app.world_mut().spawn();
        app.world_mut()
            .insert_component(workspace, Address::new("/demo").unwrap());
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new("demo".into(), root.path().into(), database),
        );
        app.world_mut()
            .insert_component(workspace, ToolTasks::new());
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            Agent::new(
                workspace,
                workspace,
                "driver-first".into(),
                root.path().into(),
            ),
        );
        app.world_mut()
            .insert_component(agent, Address::new("/demo/coder").unwrap());
        app.world_mut().insert_component(
            agent,
            open_storage(
                &root.path().join("state.sqlite"),
                &root.path().join("setting.toml"),
                "enabled = true\n",
            )
            .unwrap(),
        );
        app.world_mut().insert_component(agent, MclQueue::default());
        app.world_mut()
            .insert_component(agent, ActiveInference::default());
        app.world_mut().insert_component(agent, ToolTasks::new());
        app.world_mut().agent_ready(agent).unwrap();
        let workspace_creates = Arc::new(Mutex::new(Vec::new()));
        let recorded = workspace_creates.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            recorded
                .lock()
                .unwrap()
                .extend(world.event_reader::<CreateWorkspace>().into_iter().cloned());
        });
        let agent_starts = Arc::new(Mutex::new(Vec::new()));
        let recorded = agent_starts.clone();
        app.add_system(RuntimePlugin::UPDATE,move|world:&mut World|{recorded.lock().unwrap().extend(world.event_reader::<StartWorkspaceAgent<tool_core::ToolExecuteResponse,tool_core::ToolError>>().into_iter().cloned());});
        let inference = Arc::new(Mutex::new(Vec::new()));
        let recorded = inference.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            recorded.lock().unwrap().extend(
                world
                    .event_reader::<InferenceRequest>()
                    .into_iter()
                    .cloned(),
            );
        });
        if real_workspace {
            app.add_plugin(agent_plugin::AgentPlugin::new(
                app.world()
                    .get_resource::<agent_core::AgentUpdateContext>()
                    .unwrap()
                    .clone(),
            ));
            let root = app
                .world_mut()
                .remove_resource::<workspace_core::WorkspaceConfig>()
                .unwrap()
                .root;
            app.add_plugin(workspace_plugin::WorkspacePlugin { root });
        }
        app.add_plugin(DtoPlugin::new());
        notify_sources(app.world_mut(), agent, None);
        app.add_plugin(ui_plugin::UiPlugin::new());
        if let Some((frames, events)) = counters {
            app.add_system(RuntimePlugin::UPDATE, move |_world: &mut World| {
                frames.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let _ = &events;
            });
        }
        app.tick();
        let deadline = Instant::now() + Duration::from_secs(5);
        let server = loop {
            app.tick();
            if let Some(address) = app
                .world()
                .get_resource::<AxumHandle>()
                .and_then(AxumHandle::local_address)
            {
                break address;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        if real_workspace {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app
                .world()
                .get_resource::<workspace_core::WorkspacePendingWork>()
                .unwrap()
                .is_idle()
            {
                app.tick();
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        Self {
            app,
            runtime,
            server,
            root,
            workspace,
            agent,
            workspace_creates,
            agent_starts,
            inference,
        }
    }

    fn connect(&mut self) -> Socket {
        self.runtime
            .block_on(tokio_tungstenite::connect_async(format!(
                "ws://{}/ws",
                self.server
            )))
            .unwrap()
            .0
    }

    fn registered_connection(&self, registration: &Value) -> ConnectionId {
        let address = registration["client"]["address"].as_str().unwrap();
        self.app
            .world()
            .query_with::<Client>()
            .result()
            .into_iter()
            .find_map(|entity| {
                (self
                    .app
                    .world()
                    .get_component::<Address>(entity)
                    .unwrap()
                    .as_str()
                    == address)
                    .then(|| {
                        self.app
                            .world()
                            .get_component::<Client>(entity)
                            .unwrap()
                            .connection_id
                    })
            })
            .expect("registration must create a Client with the original connection")
    }

    // The native transport replies to Ping only after consuming the preceding
    // request. No World tick runs here: the DTO ingress is queued for UPDATE.
    fn ingress_barrier(&self, socket: &mut Socket) {
        self.runtime.block_on(async {
            socket
                .send(Message::Ping(b"ingress-barrier".to_vec().into()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match socket.next().await.unwrap().unwrap() {
                        Message::Pong(payload) if payload.as_ref() == b"ingress-barrier" => break,
                        Message::Close(frame) => {
                            panic!("socket closed before ingress barrier: {frame:?}")
                        }
                        _ => (),
                    }
                }
            })
            .await
            .expect("native websocket did not acknowledge ingress");
        });
    }

    fn send(&self, socket: &mut Socket, value: Value) {
        self.runtime
            .block_on(socket.send(Message::Text(value.to_string().into())))
            .unwrap();
    }
    fn next(&mut self, socket: &mut Socket, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            if let Ok(Some(Ok(Message::Text(text)))) = self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_millis(3), socket.next()).await
            }) {
                let value: Value = serde_json::from_str(&text).unwrap();
                if matches(&value) {
                    return value;
                }
            }
            assert!(
                Instant::now() < deadline,
                "expected websocket response did not arrive"
            );
        }
    }
    fn collect(&mut self, socket: &mut Socket, ticks: usize) -> Vec<Value> {
        let mut messages = Vec::new();
        for _ in 0..ticks {
            self.app.tick();
            while let Ok(Some(Ok(Message::Text(text)))) = self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_millis(2), socket.next()).await
            }) {
                messages.push(serde_json::from_str(&text).unwrap());
            }
        }
        messages
    }
    fn register(&mut self, socket: &mut Socket, kind: &str, name: &str) -> Value {
        self.send(socket, json!({"type": "connection.register", "id": "register", "message": {"client_type": kind, "name": name}}));
        self.next(socket, |value| {
            value["type"] == "connection.register.result"
        })
    }
    fn request(&self, socket: &mut Socket, kind: &str, id: &str, message: Value) {
        self.send(socket, json!({"type": kind, "id": id, "message": message}));
    }
    fn command(&self, socket: &mut Socket, id: &str, command: &str, arguments: Value) {
        self.request(
            socket,
            "mcl.execute",
            id,
            json!({"address": "/demo/coder", "command": command, "arguments": arguments}),
        );
    }
    fn target(&self) -> Value {
        json!({"address": "/demo/coder"})
    }
    fn wait(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(self) {
            self.app.tick();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn append(&self, content: &str, timestamp: u64) {
        let storage = self
            .app
            .world()
            .get_component::<AgentStorage>(self.agent)
            .unwrap();
        execute_storage(
            storage,
            StorageOperation::AppendHistory {
                records: vec![HistoryRecord {
                    source: None,
                    turn: None,
                    kind: "message".into(),
                    content: content.into(),
                    payload: json!({"type": "user", "content": content}),
                    created_at_ms: Some(timestamp),
                }],
            },
        )
        .unwrap();
    }
}

#[test]
fn websocket_registration_mcl_reply_and_audit_use_original_network_identity() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    let registered = harness.register(&mut socket, "webui", "browser");
    let connection = harness.registered_connection(&registered);
    assert_eq!(registered["id"], "register");
    assert_eq!(registered["success"], true);
    assert!(
        registered["client"]["address"]
            .as_str()
            .unwrap()
            .starts_with("/clients/webui/browser/")
    );
    assert!(registered["client"].get("name").is_none());
    let (client, _) = harness
        .app
        .world()
        .client_by_connection(connection)
        .unwrap();
    harness.command(&mut socket, "network-id", "CONFIG GET", json!([]));
    let reply = harness.next(&mut socket, |value| value["type"] == "mcl.execute.result");
    assert_eq!(reply["id"], "network-id");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["data"]["kind"], "config");
    assert_eq!(reply["data"]["value"]["enabled"], true);
    assert!(reply.get("source").is_none() && reply.get("target").is_none());
    assert_eq!(
        harness
            .app
            .world()
            .client_by_connection(connection)
            .unwrap()
            .0,
        client
    );
    harness.request(&mut socket, "agent.mcl.req", "audit", harness.target());
    let audit = harness.next(&mut socket, |value| value["id"] == "audit");
    assert_eq!(audit["type"], "agent.mcl.res");
    assert_eq!(audit["success"], true);
    assert!(audit["records"].as_array().unwrap().iter().any(|entry| {
        entry["payload"]["id"].as_str().unwrap().starts_with("dto:")
            && entry["payload"]["id"] != "network-id"
            && entry["source"] == registered["client"]["address"]
            && entry.get("key").is_none()
    }));
    harness.request(
        &mut socket,
        "agent.history.req",
        "messages",
        harness.target(),
    );
    let messages = harness.next(&mut socket, |value| value["id"] == "messages");
    assert_eq!(messages["type"], "agent.history.res");
    assert_eq!(messages["records"], json!([]));
}

#[test]
fn persisted_history_is_independent_from_classified_state_and_preserves_real_sequences() {
    let mut harness = Harness::new();
    harness.append("first", 1234);
    harness.append("second", 5678);
    let connection = rusqlite::Connection::open(harness.root.path().join("state.sqlite")).unwrap();
    connection
        .execute("DELETE FROM agent_messages WHERE sequence=1", [])
        .unwrap();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(
        &mut socket,
        "agent.history.req",
        "persisted",
        harness.target(),
    );
    let history = harness.next(&mut socket, |value| value["id"] == "persisted");
    assert_eq!(history["type"], "agent.history.res");
    assert_eq!(history["success"], true);
    assert_eq!(history["records"][0]["sequence"], 2);
    assert_eq!(history["records"][0]["payload"]["sequence"], 2);
    assert_eq!(history["records"][0]["payload"]["type"], "user");
    assert!(history["records"][0].get("source").is_none());
    assert!(history["records"][0].get("key").is_none());
    assert_eq!(history["records"][0]["created_at_ms"], 5678);
    harness.request(&mut socket, "workspace.req", "workspace", json!({}));
    let workspace = harness.next(&mut socket, |value| value["id"] == "workspace");
    assert!(workspace.get("histories").is_none() && workspace.get("agents").is_none());
    assert!(workspace["workspaces"][0].get("histories").is_none());
    harness.request(&mut socket, "agent.req", "agents", json!({}));
    let agents = harness.next(&mut socket, |value| value["id"] == "agents");
    assert!(agents.get("histories").is_none());
    assert!(agents["agents"][0].get("turn").is_none());
    harness
        .app
        .world_mut()
        .agent_begin_close(harness.agent)
        .unwrap();
    let state = harness.next(&mut socket, |value| {
        value["type"] == "agent.sync" && value["agents"][0]["status"] == "closing"
    });
    assert!(state.get("histories").is_none());
    connection.execute("DROP TABLE agent_messages", []).unwrap();
    harness.request(&mut socket, "agent.history.req", "broken", harness.target());
    let failure = harness.next(&mut socket, |value| value["id"] == "broken");
    assert_eq!(failure["type"], "agent.history.res");
    assert_eq!(failure["success"], false);
    assert!(failure.get("records").is_none());
    assert!(!failure.to_string().contains("agent_messages"));
}

#[test]
fn classified_sync_deduplicates_targets_and_never_leaks_to_other_clients() {
    let mut harness = Harness::new();
    let mut browser = harness.connect();
    harness.register(&mut browser, "webui", "browser");
    harness.next(&mut browser, |value| value["type"] == "agent.sync");
    harness.collect(&mut browser, 3);
    let quiet = harness.collect(&mut browser, 15);
    assert!(quiet.iter().all(|value| !matches!(
        value["type"].as_str(),
        Some("workspace.sync" | "agent.sync" | "agent.turn.sync" | "agent.task.sync")
    )));
    let mut terminal = harness.connect();
    harness.register(&mut terminal, "cli", "terminal");
    assert!(
        harness
            .collect(&mut terminal, 10)
            .iter()
            .all(|value| !value["type"].as_str().unwrap().ends_with(".sync"))
    );
    harness
        .app
        .world_mut()
        .agent_set_turn(harness.agent, Some("changed".into()))
        .unwrap();
    let turn = harness.next(&mut browser, |value| {
        value["type"] == "agent.turn.sync" && value["turn"] == "changed"
    });
    assert_eq!(turn["address"], "/demo/coder");
    assert!(turn.get("instance").is_none());
    assert!(
        harness
            .collect(&mut browser, 10)
            .iter()
            .all(|value| value["type"] != "agent.turn.sync")
    );
    assert!(
        harness
            .collect(&mut terminal, 10)
            .iter()
            .all(|value| !value["type"].as_str().unwrap().ends_with(".sync"))
    );
    let mut second = harness.connect();
    harness.register(&mut second, "webui", "second-browser");
    harness.next(&mut second, |value| value["type"] == "agent.sync");
    assert!(
        harness
            .collect(&mut browser, 10)
            .iter()
            .all(|value| value["type"] != "agent.sync")
    );
}

#[test]
fn abort_control_runs_next_frame_freezes_inference_and_only_cancels_unreturned_tools() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    let turn = harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    let (reply, mut returned_reply) = oneshot::channel();
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "background-call".into(),
        "bash".into(),
        reply,
        harness
            .app
            .world()
            .get_component::<ToolTasks>(harness.agent)
            .unwrap()
            .update_context(),
        Some(turn.clone()),
        fake_finish(&harness),
    )
    .unwrap();
    task.return_result(Ok(tool_core::ToolExecuteResponse {
        call_id: "background-call".into(),
        alias: "bash".into(),
        content: "started".into(),
        images: vec![],
        failed: false,
    }))
    .unwrap();
    assert_eq!(
        returned_reply.try_recv().unwrap().unwrap().content,
        "started"
    );
    let (reply, mut pending_reply) = oneshot::channel();
    let (pending_task, _pending_input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "pending-call".into(),
        "bash".into(),
        reply,
        harness
            .app
            .world()
            .get_component::<ToolTasks>(harness.agent)
            .unwrap()
            .update_context(),
        Some(turn.clone()),
        fake_finish(&harness),
    )
    .unwrap();
    let tasks = harness
        .app
        .world()
        .get_component::<ToolTasks>(harness.agent)
        .unwrap();
    tasks.register("background".into(), task.clone()).unwrap();
    tasks
        .register("pending".into(), pending_task.clone())
        .unwrap();
    let cancel = CancellationToken::new();
    let observed = cancel.clone();
    let frozen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let marked = frozen.clone();
    let (release, receiver) = oneshot::channel();
    let handle = harness.runtime.spawn(async move {
        receiver.await.unwrap();
    });
    harness
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(harness.agent)
        .unwrap()
        .current = Some({
        let inference = InferenceHandle {
            request_id: "inference".into(),
            turn: Some(turn.clone()),
            cancel: cancel.clone(),
            stop_stream: Arc::new(move || {
                assert!(!observed.is_cancelled());
                marked.store(true, std::sync::atomic::Ordering::Release);
            }),
            task: Default::default(),
            source_alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        };
        inference.attach_task(handle).unwrap();
        inference
    });
    harness.request(
        &mut socket,
        "agent.turn.abort",
        "abort",
        json!({"address": "/demo/coder", "turn": turn}),
    );
    harness.ingress_barrier(&mut socket);
    harness.app.tick();
    assert!(frozen.load(std::sync::atomic::Ordering::Acquire));
    assert!(cancel.is_cancelled());
    assert!(!task.cancel_token().is_cancelled());
    assert!(pending_task.cancel_token().is_cancelled());
    let aborted = pending_reply.try_recv().unwrap().unwrap();
    assert_eq!(aborted.call_id, "pending-call");
    assert_eq!(aborted.alias, "bash");
    assert_eq!(aborted.content, "Tool call aborted.");
    assert!(aborted.failed);
    let reply = harness.next(&mut socket, |value| value["id"] == "abort");
    assert_eq!(reply["type"], "agent.turn.abort.result");
    assert_eq!(reply["success"], true);
    assert!(reply.get("state").is_none());
    release.send(()).unwrap();
    let active = harness
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(harness.agent)
        .unwrap()
        .current
        .take()
        .unwrap();
    harness.runtime.block_on(active.wait_finished()).unwrap();
    let next_turn = harness
        .app
        .world()
        .get_component::<Agent>(harness.agent)
        .unwrap()
        .turn()
        .clone();
    assert!(next_turn.is_none());
    harness
        .app
        .world_mut()
        .agent_set_turn(harness.agent, None)
        .unwrap();
    let turn = harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    harness.request(
        &mut socket,
        "agent.turn.abort",
        "stale",
        json!({"address": "/demo/coder", "turn": format!("{turn}-stale")}),
    );
    harness.ingress_barrier(&mut socket);
    harness.app.tick();
    let reply = harness.next(&mut socket, |value| value["id"] == "stale");
    assert_eq!(reply["success"], false);
    assert_eq!(reply["error"]["code"], "StateRejected");
    assert!(
        harness
            .app
            .world()
            .get_component::<Agent>(harness.agent)
            .unwrap()
            .turn()
            .is_some()
    );
}

#[test]
fn oversized_history_query_fails_but_unrelated_state_queries_stay_small() {
    let mut harness = Harness::new();
    let storage = harness
        .app
        .world()
        .get_component::<AgentStorage>(harness.agent)
        .unwrap();
    execute_storage(
        storage,
        StorageOperation::AppendHistory {
            records: vec![HistoryRecord {
                source: None,
                turn: None,
                kind: "message".into(),
                content: "x".repeat(margatroid_protocol::MAX_MESSAGE_BYTES),
                payload: json!({"type": "user", "content": "x".repeat(margatroid_protocol::MAX_MESSAGE_BYTES)}),
                created_at_ms: Some(123),
            }],
        },
    )
    .unwrap();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(
        &mut socket,
        "agent.history.req",
        "large-history",
        harness.target(),
    );
    let reply = harness.next(&mut socket, |value| value["id"] == "large-history");
    assert_eq!(reply["type"], "agent.history.res");
    assert_eq!(reply["success"], false);
    assert_eq!(reply["error"]["code"], "ResponseTooLarge");
    assert!(reply.get("records").is_none());
    harness.request(&mut socket, "agent.req", "small-agents", json!({}));
    let reply = harness.next(&mut socket, |value| value["id"] == "small-agents");
    assert_eq!(reply["type"], "agent.res");
    assert_eq!(reply["success"], true);
    assert!(reply.to_string().len() < 16384);
    harness.request(&mut socket, "workspace.req", "small-workspaces", json!({}));
    let reply = harness.next(&mut socket, |value| value["id"] == "small-workspaces");
    assert_eq!(reply["success"], true);
    assert!(reply.to_string().len() < 16384);
}

#[test]
fn original_mcl_reply_keeps_request_identity_and_invalid_input_never_echoes_secrets() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    harness.app.world_mut().insert_component(
        harness.agent,
        inference_core::AgentInferenceConfig::new(
            inference_core::InferenceParameters {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
            crate::test_probe(),
        ),
    );
    harness.command(
        &mut socket,
        "pending",
        "EFFECT inference ?",
        json!([[{"type": "user", "content": "question"}]]),
    );
    harness.wait(|harness| !harness.inference.lock().unwrap().is_empty());
    let request = harness.inference.lock().unwrap()[0].clone();
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(InferenceResponse {
            reasoning: None,
            content: Some("old-private-result".into()),
            tool_calls: vec![],
            usage: None,
            stop_reason: InferenceStopReason::Completed,
        }))
        .unwrap();
    let response = harness.next(&mut socket, |value| value["id"] == "pending");
    assert_eq!(response["type"], "mcl.execute.result");
    assert_eq!(response["success"], true);
    assert!(response.get("instance").is_none());
    let secret = "PRIVATE-RAW-REQUEST";
    harness.request(
        &mut socket,
        "agent.history.req",
        "invalid",
        json!({"address": "not-an-agent"}),
    );
    let failure = harness.next(&mut socket, |value| value["id"] == "invalid");
    assert!(failure["error"]["message"].as_str().unwrap().len() < 513);
    assert!(!failure.to_string().contains(secret));
    harness
        .runtime
        .block_on(socket.send(Message::Text(format!("invalid-json:{secret}").into())))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.app.tick();
        if let Ok(Some(Ok(Message::Close(Some(frame))))) = harness
            .runtime
            .block_on(async { tokio::time::timeout(Duration::from_millis(5), socket.next()).await })
        {
            assert_eq!(u16::from(frame.code), 1007);
            assert!(!frame.reason.contains(secret));
            break;
        }
        assert!(Instant::now() < deadline);
    }
}

#[test]
fn agent_projection_reads_current_tools_sandboxes_and_public_model_fields() {
    use sandbox_resource_core::{Sandboxes, SelectedSandbox};
    use tool_resource_core::{ToolEntry, Tools};
    let mut harness = Harness::new();
    let mut tools = Tools::new();
    tools
        .insert(
            "bash".into(),
            ToolEntry {
                owner_agent: None,
                path: harness.root.path().join("private/bash/tool.toml"),
                description: "bash".into(),
                input_schema: json!({"type":"object"}),
                owner_task_id: None,
                binding: None,
            },
        )
        .unwrap();
    tools
        .insert(
            "read_file".into(),
            ToolEntry {
                owner_agent: None,
                path: harness.root.path().join("private/read-file/tool.toml"),
                description: "read file".into(),
                input_schema: json!({"type":"object"}),
                owner_task_id: None,
                binding: None,
            },
        )
        .unwrap();
    let mut sandboxes = Sandboxes::new(
        harness
            .app
            .world()
            .get_resource::<sandbox_resource_core::SandboxObserverFactory>()
            .unwrap()
            .for_owner(harness.agent)
            .0,
    );
    sandboxes.insert(
        "read_only".into(),
        harness.root.path().join("private/read-only/policy.json"),
    );
    harness
        .app
        .world_mut()
        .insert_component(harness.agent, tools);
    harness
        .app
        .world_mut()
        .insert_component(harness.agent, sandboxes);
    let selection_probe = harness
        .app
        .world()
        .get_resource::<sandbox_resource_core::SandboxObserverFactory>()
        .unwrap()
        .for_owner(harness.agent)
        .1;
    let model_probe = harness
        .app
        .world()
        .get_resource::<inference_core::ModelObserverFactory>()
        .unwrap()
        .for_owner(harness.agent);
    harness.app.world_mut().insert_component(
        harness.agent,
        sandbox_resource_core::SelectedSandbox::new(Some("read_only".into()), selection_probe),
    );
    harness.app.world_mut().insert_component(
        harness.agent,
        inference_core::AgentInferenceConfig::new(
            inference_core::InferenceParameters {
                model: "display-route".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
            model_probe,
        ),
    );
    harness
        .app
        .world_mut()
        .insert_resource(inference_core::ModelRoutes::new(
            std::collections::BTreeMap::from([(
                "display-route".into(),
                inference_core::ModelRoute {
                    model: "display-model".into(),
                    provider: "openai".into(),
                    api_type: "responses".into(),
                    base_url: "https://private-endpoint.invalid".into(),
                    api_key: "private-model-key".into(),
                    context_window_tokens: 8192,
                    thinking: Some("enabled".into()),
                    reasoning_effort: Some("high".into()),
                },
            )]),
        ));
    let waits = notify_changed(harness.app.world_mut(), harness.agent);
    assert!(waits.is_empty());
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "resource-panel");
    harness.request(&mut socket, "agent.req", "resources", json!({}));
    let state = harness.next(&mut socket, |value| value["id"] == "resources");
    assert_eq!(state["type"], "agent.res");
    assert_eq!(state["success"], true);
    let agent = &state["agents"][0];
    let tools = agent["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    for (tool, alias, description, path) in [
        (
            &tools[0],
            "bash",
            "bash",
            harness.root.path().join("private/bash/tool.toml"),
        ),
        (
            &tools[1],
            "read_file",
            "read file",
            harness.root.path().join("private/read-file/tool.toml"),
        ),
    ] {
        assert_eq!(tool["alias"], alias);
        assert_eq!(tool["description"], description);
        assert_eq!(tool["path"], path.to_string_lossy().as_ref());
        assert_eq!(tool["input_schema"], json!({"type":"object"}));
        assert!(tool["owner_task_id"].is_null());
        assert!(tool["binding"].is_null());
    }
    assert_eq!(
        agent["sandboxes"],
        json!([{"alias":"read_only","path":harness.root.path().join("private/read-only/policy.json")} ])
    );
    assert_eq!(agent["sandbox"], "read_only");
    assert_eq!(
        agent["model"],
        json!({"id":"display-route","model":"display-model","provider":"openai","api_type":"responses","context_window_tokens":8192,"thinking":"enabled","reasoning_effort":"high"})
    );
    assert!(!state.to_string().contains("private-model-key"));
    assert!(!state.to_string().contains("private-endpoint"));
    assert!(
        agent.get("turn").is_none()
            && agent.get("tasks").is_none()
            && agent.get("histories").is_none()
    );
    harness.collect(&mut socket, 5);
    let changed_path = harness.root.path().join("changed/read-only.toml");
    let added_path = harness.root.path().join("changed/write.toml");
    let sandboxes = harness
        .app
        .world_mut()
        .get_component_mut::<Sandboxes>(harness.agent)
        .unwrap();
    sandboxes.insert("read_only".into(), changed_path.clone());
    sandboxes.insert("write".into(), added_path.clone());
    harness
        .app
        .world_mut()
        .get_component_mut::<SelectedSandbox>(harness.agent)
        .unwrap()
        .set(None);
    let config = harness
        .app
        .world()
        .get_component::<inference_core::AgentInferenceConfig>(harness.agent)
        .unwrap()
        .clone();
    let routes = harness
        .app
        .world_mut()
        .get_resource_mut::<inference_core::ModelRoutes>()
        .unwrap();
    let mut entries = routes.entries().clone();
    entries
        .get_mut("display-route")
        .unwrap()
        .context_window_tokens = 16384;
    routes.replace(entries, &[config]);
    let sync = harness.next(&mut socket, |value| {
        value["type"] == "agent.sync"
            && value["agents"][0]["model"]["context_window_tokens"] == 16384
    });
    assert_eq!(
        sync["agents"][0]["sandboxes"],
        json!([{"alias":"read_only","path":changed_path},{"alias":"write","path":added_path}])
    );
    assert!(sync["agents"][0]["sandbox"].is_null());
    assert!(!sync.to_string().contains("private-model-key"));
    harness.request(&mut socket, "agent.req", "updated-resources", json!({}));
    let updated = harness.next(&mut socket, |value| value["id"] == "updated-resources");
    assert_eq!(updated["agents"], sync["agents"]);
    harness
        .app
        .world_mut()
        .remove_component::<Sandboxes>(harness.agent);
    harness
        .app
        .world_mut()
        .remove_component::<inference_core::AgentInferenceConfig>(harness.agent);
    let waits = notify_changed(harness.app.world_mut(), harness.agent);
    assert!(waits.is_empty());
    let cleared = harness.next(&mut socket, |value| {
        value["type"] == "agent.sync" && value["agents"][0]["sandboxes"] == json!([])
    });
    assert!(cleared["agents"][0]["model"].is_null());
    harness.request(&mut socket, "agent.req", "cleared-resources", json!({}));
    let query = harness.next(&mut socket, |value| value["id"] == "cleared-resources");
    assert_eq!(query["agents"], cleared["agents"]);
}

#[test]
fn message_log_and_mcl_queries_use_exclusive_ranges_and_sync_only_new_records() {
    let mut harness = Harness::new();
    for (index, content) in [(1, "first"), (2, "second"), (3, "third")] {
        harness.append(content, index);
    }
    let storage = harness
        .app
        .world()
        .get_component::<AgentStorage>(harness.agent)
        .unwrap()
        .clone();
    let append_log = |message: &str| {
        storage_core::append_agent_log(
            &storage,
            storage_core::AgentLogRecord {
                created_at_ms: Some(10),
                level: storage_core::AgentLogLevel::Info,
                source: storage_core::AgentLogSource::Driver,
                stage: "driver".into(),
                message: message.into(),
                details_json: Some(json!({"visible":true})),
                context_json: None,
                origin: None,
                raw_error: None,
                traceback: None,
            },
        )
        .unwrap()
    };
    append_log("historical log");
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.collect(&mut socket, 15);
    let quiet = harness.collect(&mut socket, 5);
    assert!(
        quiet
            .iter()
            .all(|v| v["type"] != "agent.log.sync" && v["type"] != "agent.mcl.sync")
    );
    let mut range = harness.target();
    range["after_sequence"] = json!(1);
    range["before_sequence"] = json!(3);
    range["limit"] = json!(1);
    harness.request(&mut socket, "agent.history.req", "range", range);
    let page = harness.next(&mut socket, |v| v["id"] == "range");
    assert_eq!(page["type"], "agent.history.res");
    assert_eq!(page["records"].as_array().unwrap().len(), 1);
    assert_eq!(page["records"][0]["sequence"], 2);
    assert_eq!(page["records"][0]["payload"]["content"], "second");
    assert!(page["next_before_sequence"].is_null() && page["next_after_sequence"].is_null());
    harness.request(&mut socket, "agent.log.req", "old-log", harness.target());
    let old = harness.next(&mut socket, |v| v["id"] == "old-log");
    assert_eq!(old["records"].as_array().unwrap().len(), 1);
    assert_eq!(old["records"][0]["message"], "historical log");
    let written = append_log("new log");
    let sync = harness.next(&mut socket, |v| v["type"] == "agent.log.sync");
    assert_eq!(sync["records"].as_array().unwrap().len(), 1);
    assert_eq!(sync["records"][0]["sequence"], written.sequence);
    assert_eq!(sync["records"][0]["message"], "new log");
    harness.command(&mut socket, "audit-new", "CONFIG GET", json!([]));
    let mut frames = vec![];
    let deadline = Instant::now() + Duration::from_secs(5);
    while !frames.iter().any(|v: &Value| v["id"] == "audit-new")
        || !frames.iter().any(|v| v["type"] == "agent.mcl.sync")
    {
        frames.extend(harness.collect(&mut socket, 1));
        assert!(Instant::now() < deadline);
    }
    let records = frames
        .iter()
        .filter(|v| v["type"] == "agent.mcl.sync")
        .flat_map(|v| v["records"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    let audit_sequence = records[0]["sequence"].as_u64().unwrap();
    assert_eq!(records[0]["content"], "CONFIG GET");
    let mut audit_range = harness.target();
    audit_range["after_sequence"] = json!(audit_sequence - 1);
    audit_range["before_sequence"] = json!(audit_sequence + 1);
    harness.request(&mut socket, "agent.mcl.req", "audit-range", audit_range);
    let page = harness.next(&mut socket, |v| v["id"] == "audit-range");
    assert_eq!(page["records"].as_array().unwrap().len(), 1);
    assert_eq!(page["records"][0]["sequence"], audit_sequence);
    let mut logs = harness.target();
    logs["after_sequence"] = json!(1);
    logs["before_sequence"] = json!(3);
    harness.request(&mut socket, "agent.log.req", "log-range", logs);
    let page = harness.next(&mut socket, |v| v["id"] == "log-range");
    assert_eq!(page["records"].as_array().unwrap().len(), 1);
    assert_eq!(page["records"][0]["message"], "new log");
    assert!(
        harness
            .collect(&mut socket, 15)
            .iter()
            .all(|v| v["type"] != "agent.log.sync" && v["type"] != "agent.mcl.sync")
    );
    // Existing cursor is unaffected by a new newest message.
    harness.append("newest", 20);
    let mut older = harness.target();
    older["before_sequence"] = json!(3);
    older["limit"] = json!(50);
    harness.request(&mut socket, "agent.history.req", "older", older);
    let old = harness.next(&mut socket, |v| v["id"] == "older");
    assert_eq!(
        old["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn task_queries_and_complete_sync_do_not_consume_script_output_or_replaced_marker() {
    let mut harness = Harness::new();
    let (reply, _response) = oneshot::channel();
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "task-call".into(),
        "bash".into(),
        reply,
        harness
            .app
            .world()
            .get_component::<ToolTasks>(harness.agent)
            .unwrap()
            .update_context(),
        None,
        fake_finish(&harness),
    )
    .unwrap();
    task.replace(json!("first")).unwrap();
    harness
        .app
        .world()
        .get_component::<ToolTasks>(harness.agent)
        .unwrap()
        .register("task-one".into(), task.clone())
        .unwrap();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(&mut socket, "agent.task.req", "tasks", harness.target());
    let page = harness.next(&mut socket, |v| v["id"] == "tasks");
    assert_eq!(page["type"], "agent.task.res");
    assert_eq!(page["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(page["tasks"][0]["task_id"], "task-one");
    assert_eq!(page["tasks"][0]["call_id"], "task-call");
    assert_eq!(page["tasks"][0]["alias"], "bash");
    assert_eq!(page["tasks"][0]["returned"], false);
    assert_eq!(page["tasks"][0]["output"], json!(["first"]));
    let read = task.read().unwrap();
    assert!(read.replaced);
    assert_eq!(read.output, vec![json!("first")]);
    task.write(json!("second")).unwrap();
    let sync = harness.next(&mut socket, |v| {
        v["type"] == "agent.task.sync" && v["tasks"][0]["output"] == json!(["first", "second"])
    });
    assert!(sync.get("removed").is_none());
    assert_eq!(sync["tasks"].as_array().unwrap().len(), 1);
    let read = task.read().unwrap();
    assert!(!read.replaced);
    assert_eq!(read.output, vec![json!("second")]);
    task.replace(json!({"structured":true})).unwrap();
    harness.request(
        &mut socket,
        "agent.task.req",
        "replacement",
        harness.target(),
    );
    let page = harness.next(&mut socket, |v| v["id"] == "replacement");
    assert_eq!(page["tasks"][0]["output"], json!([{"structured":true}]));
    let read = task.read().unwrap();
    assert!(read.replaced);
    assert_eq!(read.output, vec![json!({"structured":true})]);
    assert!(!task.cancel_token().is_cancelled());
}

#[test]
fn native_service_shutdown_closes_registered_and_unregistered_connections() {
    let mut harness = Harness::new();
    let mut registered = harness.connect();
    harness.register(&mut registered, "webui", "shutdown-browser");
    let _unregistered = harness.connect();
    let handle = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    handle.request_shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_stopped()
        || !harness
            .app
            .world()
            .query_with::<client_core::Client>()
            .result()
            .is_empty()
    {
        harness.app.tick();
        assert!(
            Instant::now() < deadline,
            "native service did not close connections and registered identities"
        );
        std::thread::yield_now();
    }
    assert!(handle.result().unwrap().is_ok());
    harness.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match registered.next().await {
                    None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .expect("registered socket did not close after pending frames");
    });
}

#[test]
fn driver_ui_query_and_sync_follow_actual_source_and_recover_from_resource_errors() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "driver-ui");
    let source = harness.root.path().join("loaded-driver");
    std::fs::create_dir_all(source.join("ui")).unwrap();
    std::fs::write(source.join("ui/index.html"), "first").unwrap();
    notify_sources(
        harness.app.world_mut(),
        harness.agent,
        Some(source.join("ui")),
    );
    let waits = notify_changed(harness.app.world_mut(), harness.agent);
    harness.runtime.block_on(async {
        for wait in waits {
            wait.await.unwrap();
        }
    });
    let initial = harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["ui"]["entry"] == "index.html"
    });
    assert_eq!(initial["ui"]["files"][0]["data"], "Zmlyc3Q=");
    assert_eq!(initial["address"], "/demo/coder");
    harness.request(&mut socket, "agent.ui.req", "ui-read", harness.target());
    let response = harness.next(&mut socket, |v| v["id"] == "ui-read");
    assert_eq!(response["ui"], initial["ui"]);
    assert_eq!(response["success"], true);
    assert!(
        harness
            .collect(&mut socket, 5)
            .iter()
            .all(|v| v["type"] != "agent.ui.sync")
    );
    std::fs::create_dir_all(source.join("ui/assets")).unwrap();
    std::fs::write(source.join("ui/assets/app.js"), "export const value=1").unwrap();
    let added = harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync"
            && v["ui"]["files"]
                .as_array()
                .is_some_and(|files| files.len() == 2)
    });
    assert_ne!(added["ui"]["revision"], initial["ui"]["revision"]);
    let entry = source.join("ui/index.html");
    assert!(entry.starts_with(harness.root.path()));
    std::fs::remove_file(&entry).unwrap();
    let failure = harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["error"]["code"] == "DriverUiEntryMissing"
    });
    assert!(failure["ui"].is_null());
    harness.request(&mut socket, "agent.ui.req", "ui-failure", harness.target());
    let failure = harness.next(&mut socket, |v| v["id"] == "ui-failure");
    assert_eq!(failure["success"], false);
    assert!(failure.get("ui").is_none());
    std::fs::write(&entry, "first").unwrap();
    let recovered = harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["ui"]["revision"] == added["ui"]["revision"]
    });
    assert!(recovered["error"].is_null());
    let replacement = harness.root.path().join("replacement-driver");
    std::fs::create_dir_all(replacement.join("ui")).unwrap();
    std::fs::write(replacement.join("ui/index.html"), "second").unwrap();
    notify_sources(
        harness.app.world_mut(),
        harness.agent,
        Some(replacement.join("ui")),
    );
    let waits = notify_changed(harness.app.world_mut(), harness.agent);
    harness.runtime.block_on(async {
        for wait in waits {
            wait.await.unwrap();
        }
    });
    harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["ui"]["files"][0]["data"] == "c2Vjb25k"
    });
    std::fs::write(&entry, "old source").unwrap();
    assert!(
        harness
            .collect(&mut socket, 10)
            .iter()
            .all(|v| v["type"] != "agent.ui.sync")
    );
    let ui = replacement.join("ui");
    assert!(ui.starts_with(harness.root.path()));
    std::fs::remove_dir_all(ui).unwrap();
    harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["ui"].is_null() && v["error"].is_null()
    });
    harness.request(&mut socket, "agent.ui.req", "ui-absent", harness.target());
    let absent = harness.next(&mut socket, |v| v["id"] == "ui-absent");
    assert_eq!(absent["success"], true);
    assert!(absent["ui"].is_null());
    let server = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    server.request_shutdown();
    harness.wait(|_| server.is_stopped());
    assert!(server.result().unwrap().is_ok());
}

#[test]
fn driver_ui_change_advances_an_idle_event_driven_world_without_polling_queries() {
    use app_runtime_plugin::{AppRunExt, RuntimeHandle};
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut harness = Harness::new();
    harness
        .app
        .world_mut()
        .remove_component::<AgentStorage>(harness.agent);
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "ui-event-loop");
    let source = harness.root.path().join("event-driver");
    std::fs::create_dir_all(source.join("ui")).unwrap();
    let entry = source.join("ui/index.html");
    std::fs::write(&entry, "before").unwrap();
    notify_sources(
        harness.app.world_mut(),
        harness.agent,
        Some(source.join("ui")),
    );
    let waits = notify_changed(harness.app.world_mut(), harness.agent);
    harness.runtime.block_on(async {
        for wait in waits {
            wait.await.unwrap();
        }
    });
    harness.next(&mut socket, |v| {
        v["type"] == "agent.ui.sync" && v["ui"].is_object()
    });
    let stopping = Arc::new(AtomicBool::new(false));
    let stopped = stopping.clone();
    let handle = harness
        .app
        .world()
        .get_resource::<RuntimeHandle>()
        .unwrap()
        .clone();
    let runtime = harness.runtime;
    let worker = std::thread::spawn(move || {
        struct Stop(Arc<AtomicBool>, RuntimeHandle);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                self.1.wake();
            }
        }
        let _stop = Stop(stopped, handle);
        runtime.block_on(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            std::fs::write(entry, "after").unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                        continue;
                    };
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "agent.ui.sync"
                        && value["ui"]["files"][0]["data"] == "YWZ0ZXI="
                    {
                        break;
                    }
                }
            })
            .await
            .expect("file update did not advance event-driven synchronization");
            socket.close(None).await.unwrap();
        });
    });
    harness.app.run_until(|| stopping.load(Ordering::Acquire));
    worker.join().unwrap();
    let server = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    server.request_shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !server.is_stopped() {
        harness.app.tick();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn record_sync_sleeps_when_idle_and_one_commit_does_not_start_a_frame_feedback_loop() {
    use app_runtime_plugin::{AppRunExt, RuntimeHandle};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let frames = Arc::new(AtomicUsize::new(0));
    let outbound = Arc::new(AtomicUsize::new(0));
    let mut harness = Harness::with_counters(Some((frames.clone(), outbound.clone())));
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "idle-record-test");
    harness.collect(&mut socket, 5);
    frames.store(0, Ordering::Release);
    outbound.store(0, Ordering::Release);
    let storage = harness
        .app
        .world()
        .get_component::<AgentStorage>(harness.agent)
        .unwrap()
        .clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let runtime = harness
        .app
        .world()
        .get_resource::<RuntimeHandle>()
        .unwrap()
        .clone();
    let observed = frames.clone();
    let worker = std::thread::spawn(move || {
        struct Finish(Arc<AtomicBool>, RuntimeHandle);
        impl Drop for Finish {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                self.1.wake();
            }
        }
        let _finish = Finish(stopping, runtime);
        std::thread::sleep(Duration::from_millis(150));
        let idle = observed.load(Ordering::Acquire);
        storage_core::append_message_history(
            &storage,
            &[json!({"type":"user","content":"one actual update"})],
            None,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let after = observed.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(150));
        let settled = observed.load(Ordering::Acquire);
        (idle, after, settled)
    });
    harness.app.run_until(|| stop.load(Ordering::Acquire));
    let (idle, after, settled) = worker.join().unwrap();
    assert!(idle <= 3, "idle frame loop: {idle}");
    assert_eq!(
        after, idle,
        "record update unexpectedly advanced World frames"
    );
    assert_eq!(after, settled, "record completion kept the loop running");
    assert_eq!(outbound.load(Ordering::Acquire), 0);
    let value = harness.next(&mut socket, |value| {
        value["type"] == "agent.message.full.sync"
    });
    assert_eq!(value["message"]["content"], "one actual update");
    let server = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    server.request_shutdown();
    harness.wait(|_| server.is_stopped());
}

#[test]
fn task_output_return_and_cleanup_reach_websocket_without_any_world_update_frames() {
    use app_runtime_plugin::{AppRunExt, RuntimeHandle};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let frames = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(AtomicUsize::new(0));
    let mut harness = Harness::with_counters(Some((frames.clone(), events.clone())));
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "task-direct");
    harness.collect(&mut socket, 5);
    frames.store(0, Ordering::Release);
    events.store(0, Ordering::Release);
    let tasks = harness
        .app
        .world()
        .get_component::<ToolTasks>(harness.agent)
        .unwrap()
        .clone();
    let (reply, _response) = oneshot::channel();
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "direct-call".into(),
        "bash".into(),
        reply,
        tasks.update_context(),
        None,
        fake_finish(&harness),
    )
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let runtime = harness
        .app
        .world()
        .get_resource::<RuntimeHandle>()
        .unwrap()
        .clone();
    let counter = frames.clone();
    let worker = std::thread::spawn(move || {
        struct Finish(Arc<AtomicBool>, RuntimeHandle);
        impl Drop for Finish {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
                self.1.wake();
            }
        }
        let _finish = Finish(stopping, runtime);
        std::thread::sleep(Duration::from_millis(150));
        let before = counter.load(Ordering::Acquire);
        tasks.register("direct".into(), task.clone()).unwrap();
        task.write(json!("output without frames")).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        task.return_result(Ok(tool_core::parse_tool_result(&json!("done")).unwrap()))
            .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(tasks.complete_cleanup("direct", &task, None));
        std::thread::sleep(Duration::from_millis(150));
        (before, counter.load(Ordering::Acquire))
    });
    harness.app.run_until(|| stop.load(Ordering::Acquire));
    let (before, after) = worker.join().unwrap();
    assert_eq!(before, after, "Task updates drove World frames");
    assert_eq!(
        events.load(Ordering::Acquire),
        0,
        "Task updates emitted sync events"
    );
    let values = harness.collect(&mut socket, 5);
    assert!(values.iter().any(|v| v["type"] == "agent.task.sync"
        && v["tasks"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["output"] == json!(["output without frames"]))
        })));
    assert!(values.iter().any(|v| {
        v["type"] == "agent.task.sync"
            && v["tasks"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["returned"] == true))
    }));
    assert!(
        values
            .iter()
            .any(|v| v["type"] == "agent.task.sync" && v["tasks"] == json!([]))
    );
    let server = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    server.request_shutdown();
    harness.wait(|_| server.is_stopped());
}

fn test_probe<T: Send + 'static>() -> Arc<dyn Fn(T) + Send + Sync> {
    Arc::new(|_| {})
}
fn fake_finish(h: &Harness) -> tool_core::TaskFinish {
    let tasks = h
        .app
        .world()
        .get_component::<ToolTasks>(h.agent)
        .unwrap()
        .clone();
    Arc::new(move |handle| {
        let tasks = tasks.clone();
        Box::pin(async move {
            handle.cancel();
            handle.finish_children().await?;
            handle.retry_cleanups().await?;
            let id = tasks
                .list()
                .into_iter()
                .find(|(_, current)| current.same_task(&handle))
                .map(|(id, _)| id)
                .unwrap_or_default();
            tasks.complete_cleanup(&id, &handle, None);
            Ok(())
        })
    })
}

fn notify_changed(world: &mut World, owner: Entity) -> Vec<agent_core::AgentObservationWait> {
    let context = world
        .get_resource::<agent_core::AgentUpdateContext>()
        .unwrap()
        .clone();
    context.notify(world, owner, agent_core::AgentUpdateKind::Changed)
}
fn notify_sources(world: &mut World, owner: Entity, ui_path: Option<std::path::PathBuf>) {
    let context = world
        .get_resource::<agent_core::AgentUpdateContext>()
        .unwrap()
        .clone();
    let waits = context.notify(
        world,
        owner,
        agent_core::AgentUpdateKind::SourcesInstalled { ui_path },
    );
    if !waits.is_empty() {
        world
            .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
            .unwrap()
            .spawn(async move {
                for wait in waits {
                    wait.await.unwrap();
                }
            });
    }
}

#[test]
fn turn_and_resource_mutations_publish_without_another_world_tick() {
    use sandbox_resource_core::{Sandboxes, SelectedSandbox};
    use tool_resource_core::{ToolEntry, Tools};
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "sync-probes");
    harness.collect(&mut socket, 5);
    let mut tools = Tools::new();
    let callback = dto_plugin::resource_sync_bindings(
        harness
            .app
            .world()
            .get_resource::<agent_core::AgentUpdateContext>()
            .unwrap()
            .clone(),
    )
    .for_owner(harness.agent);
    tools.observe(move |value| callback(value));
    harness
        .app
        .world_mut()
        .insert_component(harness.agent, tools);
    let sandboxes = Sandboxes::new(
        harness
            .app
            .world()
            .get_resource::<sandbox_resource_core::SandboxObserverFactory>()
            .unwrap()
            .for_owner(harness.agent)
            .0,
    );
    harness
        .app
        .world_mut()
        .insert_component(harness.agent, sandboxes);
    let selected = SelectedSandbox::new(
        None,
        harness
            .app
            .world()
            .get_resource::<sandbox_resource_core::SandboxObserverFactory>()
            .unwrap()
            .for_owner(harness.agent)
            .1,
    );
    harness
        .app
        .world_mut()
        .insert_component(harness.agent, selected);
    let turn = harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    let path = harness.root.path().join("probe/tool.toml");
    harness
        .app
        .world_mut()
        .get_component_mut::<Tools>(harness.agent)
        .unwrap()
        .insert(
            "live_tool".into(),
            ToolEntry {
                owner_agent: None,
                path,
                description: "probe".into(),
                input_schema: json!({"type":"object"}),
                owner_task_id: None,
                binding: None,
            },
        )
        .unwrap();
    let path = harness.root.path().join("probe/policy.json");
    harness
        .app
        .world_mut()
        .get_component_mut::<Sandboxes>(harness.agent)
        .unwrap()
        .insert("probe".into(), path);
    sandbox_resource_core::WorldSandboxResourceExt::sandbox_select(
        harness.app.world_mut(),
        harness.agent,
        Some("probe".into()),
    )
    .unwrap();
    harness.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut saw_turn = false;
            let mut saw_agent = false;
            while !saw_turn || !saw_agent {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                saw_turn |= value["type"] == "agent.turn.sync" && value["turn"] == turn;
                saw_agent |= value["type"] == "agent.sync"
                    && value["agents"][0]["tools"][0]["alias"] == "live_tool"
                    && value["agents"][0]["sandbox"] == "probe";
            }
        })
        .await
        .unwrap();
    });
    let server = harness
        .app
        .world()
        .get_resource::<AxumHandle>()
        .unwrap()
        .clone();
    server.request_shutdown();
    harness.wait(|_| server.is_stopped());
}

#[test]
fn aborting_user_turn_preserves_free_inference_and_free_pending_tool() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "free-abort");
    let turn = harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    let (tx, rx) = oneshot::channel();
    let handle = InferenceHandle {
        request_id: "free-http".into(),
        turn: None,
        cancel: CancellationToken::new(),
        stop_stream: Arc::new(|| panic!("free stream must not be frozen by turn abort")),
        task: Default::default(),
        source_alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    handle
        .attach_task(harness.runtime.spawn(async move {
            rx.await.unwrap();
        }))
        .unwrap();
    harness
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(harness.agent)
        .unwrap()
        .current = Some(handle.clone());
    let tasks = harness
        .app
        .world()
        .get_component::<ToolTasks>(harness.agent)
        .unwrap()
        .clone();
    let (reply, mut result) = oneshot::channel();
    let (tool, _) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "free-call".into(),
        "free-tool".into(),
        reply,
        tasks.update_context(),
        None,
        fake_finish(&harness),
    )
    .unwrap();
    tasks.register("free-task".into(), tool.clone()).unwrap();
    harness.request(
        &mut socket,
        "agent.turn.abort",
        "abort-free",
        json!({"address":"/demo/coder","turn":turn}),
    );
    let reply = harness.next(&mut socket, |value| value["id"] == "abort-free");
    assert_eq!(reply["success"], true);
    assert!(!handle.cancel.is_cancelled());
    assert!(!tool.cancel_token().is_cancelled());
    assert!(!tool.has_returned());
    assert!(result.try_recv().is_err());
    tool.return_result(Ok(tool_core::ToolExecuteResponse {
        call_id: "free-call".into(),
        alias: "free-tool".into(),
        content: "done".into(),
        images: vec![],
        failed: false,
    }))
    .unwrap();
    assert!(!result.try_recv().unwrap().unwrap().failed);
    tasks.complete_cleanup("free-task", &tool, None);
    tx.send(()).unwrap();
    harness.runtime.block_on(handle.wait_finished()).unwrap();
    harness
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(harness.agent)
        .unwrap()
        .current
        .take();
}

struct FileHttpResponse {
    status: u16,
    headers: std::collections::BTreeMap<String, String>,
    body: Vec<u8>,
}
impl Harness {
    fn file_http(&mut self, method: &str, target: &str, headers: &str) -> FileHttpResponse {
        let server = self.server;
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {server}\r\nConnection: close\r\n{headers}\r\n"
        );
        let task = self.runtime.spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut socket = tokio::net::TcpStream::connect(server).await.unwrap();
            socket.write_all(request.as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(6);
        while !task.is_finished() {
            self.app.tick();
            assert!(
                Instant::now() < deadline,
                "HTTP file request did not complete"
            );
            self.runtime.block_on(async {
                tokio::time::sleep(Duration::from_millis(1)).await;
            });
        }
        let bytes = self.runtime.block_on(task).unwrap();
        let end = bytes
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap();
        let text = std::str::from_utf8(&bytes[..end]).unwrap();
        let mut lines = text.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        FileHttpResponse {
            status,
            headers,
            body: bytes[end + 4..].to_vec(),
        }
    }
}
const FILE_ROOT: &str = "/%2Fdemo";
#[cfg(unix)]
#[test]
fn workspace_file_get_head_and_updates_use_actual_working_directory_without_agent_or_registration()
{
    let mut h = Harness::new();
    h.app.world_mut().remove_component::<Agent>(h.agent);
    let image = b"\x89PNG\r\n\x1a\nactual-project-image";
    std::fs::write(h.root.path().join("mikuji.png"), image).unwrap();
    std::fs::create_dir_all(h.root.path().join("docs")).unwrap();
    std::fs::write(h.root.path().join("docs/中文 图.png"), b"unicode-image").unwrap();
    let response = h.file_http("GET", &format!("{FILE_ROOT}/mikuji.png"), "");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, image);
    assert_eq!(response.headers["content-type"], "image/png");
    assert_eq!(response.headers["content-length"], image.len().to_string());
    assert_eq!(response.headers["cache-control"], "no-store");
    assert_eq!(response.headers["x-content-type-options"], "nosniff");
    let head = h.file_http("HEAD", &format!("{FILE_ROOT}/mikuji.png"), "");
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.headers["content-length"], image.len().to_string());
    let ranged = h.file_http(
        "GET",
        &format!("{FILE_ROOT}/mikuji.png?v=2"),
        "Range: bytes=0-1\r\n",
    );
    assert_eq!(ranged.status, 200);
    assert_eq!(ranged.body, image);
    let unicode = h.file_http(
        "GET",
        &format!("{FILE_ROOT}/docs/%E4%B8%AD%E6%96%87%20%E5%9B%BE.png"),
        "",
    );
    assert_eq!(unicode.status, 200);
    assert_eq!(unicode.body, b"unicode-image");
    let wrong = h.root.path().join("new-definition-root");
    std::fs::create_dir(&wrong).unwrap();
    std::fs::write(wrong.join("mikuji.png"), b"wrong-definition").unwrap();
    std::fs::write(h.root.path().join("mikuji.png"), b"changed-actual-root").unwrap();
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/mikuji.png"), "")
            .body,
        b"changed-actual-root"
    );
    assert!(h.app.world().query_with::<Client>().result().is_empty());
    assert!(
        h.app
            .world()
            .get_component::<MclQueue>(h.agent)
            .unwrap()
            .commands
            .is_empty()
    );
}
#[cfg(unix)]
#[test]
fn workspace_file_paths_are_decoded_once_and_cannot_escape_or_follow_symlinks() {
    let mut h = Harness::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"outside-secret").unwrap();
    std::fs::write(h.root.path().join("inside"), b"inside").unwrap();
    std::fs::create_dir(h.root.path().join("folder")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), h.root.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path(), h.root.path().join("linkdir")).unwrap();
    std::os::unix::fs::symlink(
        h.root.path().join("inside"),
        h.root.path().join("inside-link"),
    )
    .unwrap();
    for (path, status) in [
        ("../secret", 403),
        ("%2e%2e/secret", 403),
        ("folder/%2E%2E/inside", 403),
        ("%2Fetc/passwd", 400),
        ("a%5Cb", 400),
        ("%00", 400),
        ("bad%zz", 400),
        ("bad%", 400),
        ("%FF", 400),
        ("./inside", 400),
        ("folder//inside", 400),
        ("C%3A/foo", 400),
        ("link", 403),
        ("linkdir/secret", 403),
        ("inside-link", 403),
        ("folder", 404),
        ("absent", 404),
        ("inside/no-child", 404),
    ] {
        let response = h.file_http("GET", &format!("{FILE_ROOT}/{path}"), "");
        assert_eq!(response.status, status, "{path}");
        assert!(
            response.body.is_empty(),
            "{path}: errors must not expose paths"
        );
    }
    std::fs::create_dir(h.root.path().join("%2e%2e")).unwrap();
    std::fs::write(
        h.root.path().join("%2e%2e/inside"),
        b"literal-percent-directory",
    )
    .unwrap();
    let literal = h.file_http("GET", &format!("{FILE_ROOT}/%252e%252e/inside"), "");
    assert_eq!(literal.status, 200);
    assert_eq!(literal.body, b"literal-percent-directory");
    let fifo = h.root.path().join("pipe");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc_mkfifo_for_test(c.as_ptr()) }, 0);
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/pipe"), "").status,
        404
    );
}
#[cfg(unix)]
unsafe fn libc_mkfifo_for_test(path: *const std::ffi::c_char) -> std::ffi::c_int {
    unsafe extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> std::ffi::c_int;
    }
    unsafe { mkfifo(path, 0o600) }
}
#[cfg(unix)]
#[test]
fn workspace_file_identity_lifecycle_and_ingress_are_checked_per_request() {
    let mut h = Harness::new();
    std::fs::write(h.root.path().join("sample.txt"), b"sample").unwrap();
    assert_eq!(h.file_http("GET", "/demo/sample.txt", "").status, 400);
    assert_eq!(
        h.file_http("GET", "/workspace%3Amargatroid%2Fdemo/sample.txt", "")
            .status,
        400
    );
    assert_eq!(
        h.file_http("GET", "/agent%3Ademo%2Fcoder%3Adev/sample.txt", "")
            .status,
        400
    );
    assert_eq!(h.file_http("GET", "/%2Fabsent/sample.txt", "").status, 404);
    assert_eq!(
        h.file_http(
            "GET",
            &format!("{FILE_ROOT}/sample.txt"),
            "Origin: http://example.invalid\r\n"
        )
        .status,
        403
    );
    assert_eq!(
        h.file_http(
            "GET",
            &format!("{FILE_ROOT}/sample.txt"),
            "Origin: null\r\n"
        )
        .status,
        403
    );
    for state in [WorkspaceState::Running, WorkspaceState::Error] {
        h.app
            .world_mut()
            .get_component_mut::<workspace_core::Workspace>(h.workspace)
            .unwrap()
            .set_state(state);
        assert_eq!(
            h.file_http("GET", &format!("{FILE_ROOT}/sample.txt"), "")
                .status,
            200
        );
    }
    h.app
        .world_mut()
        .get_component_mut::<workspace_core::Workspace>(h.workspace)
        .unwrap()
        .set_state(WorkspaceState::Deleting);
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/sample.txt"), "")
            .status,
        409
    );
    h.app
        .world_mut()
        .remove_component::<workspace_core::Workspace>(h.workspace);
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/sample.txt"), "")
            .status,
        404
    );
    let mut denied =
        Harness::with_counters_and_ingress(None, config_core::IngressPolicy::Addresses(vec![]));
    assert_eq!(
        denied
            .file_http("GET", &format!("{FILE_ROOT}/sample.txt"), "")
            .status,
        403
    );
}
#[test]
fn platform_assets_and_websocket_keep_priority_over_workspace_file_route() {
    let mut h = Harness::new();
    let page = h.file_http("GET", "/", "");
    assert_eq!(page.status, 200);
    let html = String::from_utf8(page.body).unwrap();
    let script = html
        .split("src=\"")
        .skip(1)
        .map(|tail| tail.split('"').next().unwrap())
        .find(|path| path.starts_with("/assets/") && path.ends_with(".js"))
        .unwrap();
    let asset = h.file_http("GET", script, "");
    assert_eq!(asset.status, 200);
    assert!(asset.headers["content-type"].contains("javascript"));
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "file-route-priority");
}

#[cfg(unix)]
#[test]
fn workspace_files_use_complete_addresses_when_display_names_are_equal() {
    let mut h = Harness::new();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(h.root.path().join("same.png"), b"first-workspace").unwrap();
    std::fs::write(second.path().join("same.png"), b"second-workspace").unwrap();
    let entity = h.app.world_mut().spawn();
    h.app
        .world_mut()
        .insert_component(entity, Address::new("/other").unwrap());
    let managed = second.path().join("managed");
    for kind in ["normal", "temporary", "archived"] {
        std::fs::create_dir_all(managed.join("agents").join(kind)).unwrap();
    }
    let database = WorkspaceDatabase::create(
        managed.join("workspace.sqlite"),
        WorkspaceMetadata {
            address: "/other".into(),
            alias: "demo".into(),
            project_root: second.path().into(),
        },
    )
    .unwrap();
    h.app.world_mut().insert_component(
        entity,
        workspace_core::Workspace::new("demo".into(), second.path().into(), database),
    );
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/same.png"), "")
            .body,
        b"first-workspace"
    );
    assert_eq!(
        h.file_http("GET", "/%2Fother/same.png", "").body,
        b"second-workspace"
    );
    let origin = format!("Origin: http://{}\r\n", h.server);
    assert_eq!(
        h.file_http("GET", &format!("{FILE_ROOT}/same.png"), &origin)
            .status,
        200
    );
    assert_eq!(
        h.file_http("POST", &format!("{FILE_ROOT}/same.png"), "")
            .status,
        405
    );
}

#[test]
fn generated_error_history_is_queryable_without_a_second_full_and_independent_errors_still_sync() {
    let mut h = Harness::new();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "error-once");
    let storage = h
        .app
        .world()
        .get_component::<AgentStorage>(h.agent)
        .unwrap()
        .clone();
    let messages = json!([{"type":"error","message":"provider refused","request_id":"inference-failed"},{"type":"error","message":"independent error"},{"type":"compact","request_id":"summary-success","content":"summary","tool_calls":[]}]);
    let saved =
        storage_core::append_message_history(&storage, messages.as_array().unwrap(), None).unwrap();
    let mut frames = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !frames
        .iter()
        .any(|v: &Value| v["message"]["type"] == "compact")
    {
        frames.extend(h.collect(&mut socket, 1));
        assert!(Instant::now() < deadline);
    }
    frames.extend(h.collect(&mut socket, 10));
    assert!(
        !frames.iter().any(|v| v["type"] == "agent.message.full.sync"
            && v["message"]["request_id"] == "inference-failed")
    );
    assert!(
        frames
            .iter()
            .any(|v| v["message"]["message"] == "independent error")
    );
    assert!(frames.iter().any(|v| v["message"]["type"] == "compact"));
    h.request(&mut socket, "agent.history.req", "history", h.target());
    let history = h.next(&mut socket, |v| v["id"] == "history");
    assert!(
        history["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["payload"]["request_id"] == "inference-failed"
                && row["sequence"] == saved[0]["sequence"])
    );
}

fn workspace_database(h: &Harness) -> WorkspaceDatabase {
    h.app
        .world()
        .get_component::<workspace_core::Workspace>(h.workspace)
        .unwrap()
        .database
        .clone()
}
fn register_record(h: &Harness, node: &str) -> workspace_core::WorkspaceAgent {
    let db = workspace_database(h);
    let operation = db.try_operation(node).unwrap();
    let candidate = db
        .prepare_agent(
            &operation,
            "/demo".into(),
            Some(node.into()),
            Some("worker".into()),
            h.root.path().join("driver/tool.toml"),
            json!({"node_name":node,"path":"."}),
        )
        .unwrap();
    std::fs::create_dir_all(db.agent_directory(&candidate).unwrap()).unwrap();
    db.commit_agent(&operation, candidate).unwrap()
}
fn tool_reply(
    request: &StartWorkspaceAgent<tool_core::ToolExecuteResponse, tool_core::ToolError>,
) -> workspace_core::WorkspaceAgentReply<tool_core::ToolExecuteResponse, tool_core::ToolError> {
    match request {
        StartWorkspaceAgent::Existing { reply, .. } | StartWorkspaceAgent::New { reply, .. } => {
            reply.clone()
        }
    }
}
fn complete_driver(
    request: &StartWorkspaceAgent<tool_core::ToolExecuteResponse, tool_core::ToolError>,
) {
    tool_reply(request)
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(tool_core::ToolExecuteResponse {
            call_id: "driver-task".into(),
            alias: "driver".into(),
            content: "started".into(),
            images: vec![],
            failed: false,
        }))
        .unwrap();
}
#[test]
fn startup_variants_preserve_driver_arguments_and_original_reply_without_world_frames() {
    let mut h = Harness::new();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "startup");
    let path = h.root.path().join("driver/tool.toml");
    let arguments = json!({"node_name":"new","alias":"worker","path":"src","custom":{"value":3}});
    h.request(
        &mut socket,
        "agent.start",
        "new",
        json!({"mode":"new","workspace":"/demo","tool_path":path,"arguments":arguments}),
    );
    h.wait(|h| h.agent_starts.lock().unwrap().len() == 1);
    let request = h.agent_starts.lock().unwrap()[0].clone();
    match &request {
        StartWorkspaceAgent::New {
            workspace,
            tool_path,
            arguments: actual,
            ..
        } => {
            assert_eq!(*workspace, h.workspace);
            assert_eq!(tool_path, &path);
            assert_eq!(actual, &arguments);
        }
        _ => panic!("wrong startup variant"),
    }
    complete_driver(&request);
    let response = h.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(Ok(Message::Text(text))) = socket.next().await {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["id"] == "new" {
                        break value;
                    }
                }
            }
        })
        .await
        .unwrap()
    });
    assert_eq!(
        response,
        json!({"type":"agent.start.result","id":"new","success":true})
    );
    h.request(
        &mut socket,
        "agent.start",
        "existing",
        json!({"mode":"existing","workspace":"/demo","node_name":"coder"}),
    );
    h.wait(|h| h.agent_starts.lock().unwrap().len() == 2);
    let request = h.agent_starts.lock().unwrap()[1].clone();
    assert!(
        matches!(&request,StartWorkspaceAgent::Existing{workspace,node_name,..}if *workspace==h.workspace && node_name=="coder")
    );
    complete_driver(&request);
    assert_eq!(
        h.next(&mut socket, |v| v["id"] == "existing")["success"],
        true
    );
}
#[test]
fn same_request_id_on_two_connections_and_late_disconnected_reply_are_isolated() {
    let mut h = Harness::new();
    let mut first = h.connect();
    h.register(&mut first, "webui", "first");
    let mut second = h.connect();
    h.register(&mut second, "webui", "second");
    for (socket, node) in [(&mut first, "first"), (&mut second, "second")] {
        h.request(
            socket,
            "agent.start",
            "same",
            json!({"mode":"existing","workspace":"/demo","node_name":node}),
        );
    }
    h.wait(|h| h.agent_starts.lock().unwrap().len() == 2);
    let requests = h.agent_starts.lock().unwrap().clone();
    let by_node = |node: &str| {
        requests
            .iter()
            .find(|r| matches!(r,StartWorkspaceAgent::Existing{node_name,..}if node_name==node))
            .unwrap()
    };
    complete_driver(by_node("first"));
    assert_eq!(h.next(&mut first, |v| v["id"] == "same")["success"], true);
    assert!(
        h.collect(&mut second, 3)
            .iter()
            .all(|v| v["type"] != "agent.start.result")
    );
    h.runtime.block_on(second.close(None)).unwrap();
    drop(second);
    h.wait(|h| h.app.world().query_with::<Client>().result().len() == 1);
    let sender = tool_reply(by_node("second"))
        .lock()
        .unwrap()
        .take()
        .unwrap();
    let _ = sender.send(Err(tool_core::ToolError::new(
        tool_core::ToolErrorKind::ExecutionFailed,
        "late",
    )));
    assert!(
        h.collect(&mut first, 5)
            .iter()
            .all(|v| v["type"] != "agent.start.result")
    );
}
#[test]
fn removed_workspace_and_unknown_messages_close_without_dispatching_creation() {
    let mut h = Harness::new();
    for kind in [
        "workspace.start",
        "workspace.stop",
        "workspace.restart",
        "workspace.update",
        "workspace.rename",
        "agent.restart",
        "unknown.command",
    ] {
        let mut socket = h.connect();
        h.register(&mut socket, "webui", kind);
        h.request(&mut socket, kind, "invalid", json!({}));
        h.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match socket.next().await {
                        Some(Ok(Message::Close(Some(frame)))) => {
                            assert_eq!(u16::from(frame.code), 1007);
                            break;
                        }
                        Some(Ok(_)) => {}
                        other => panic!("expected protocol close {other:?}"),
                    }
                }
            })
            .await
            .unwrap();
        });
    }
    assert!(h.workspace_creates.lock().unwrap().is_empty());
    assert!(h.agent_starts.lock().unwrap().is_empty());
}
#[test]
fn workspace_create_delete_use_real_database_and_complete_before_original_reply() {
    let mut h = Harness::with_real_workspace();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "workspace-crud");
    let project = tempfile::tempdir().unwrap();
    h.request(
        &mut socket,
        "workspace.create",
        "create",
        json!({"address":"/fresh","alias":"fresh","project_root":project.path()}),
    );
    let created = h.next(&mut socket, |v| v["id"] == "create");
    assert_eq!(created["success"], true, "{created}");
    let managed = h.root.path().join("workspaces/fresh");
    assert!(managed.join("workspace.sqlite").is_file());
    h.request(&mut socket, "workspace.req", "list", json!({}));
    let response = h.next(&mut socket, |v| v["id"] == "list");
    let row = response["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["address"] == "/fresh")
        .unwrap();
    assert_eq!(row["agents"], json!([]));
    assert!(row.get("definition").is_none());
    h.request(
        &mut socket,
        "workspace.delete",
        "delete",
        json!({"address":"/fresh"}),
    );
    assert_eq!(
        h.next(&mut socket, |v| v["id"] == "delete")["success"],
        true
    );
    assert!(!managed.exists());
    assert!(h.root.path().is_dir());
}
#[test]
fn database_commit_watch_updates_idle_websocket_without_world_frames_and_ignores_prepare() {
    let mut h = Harness::new();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "database-watch");
    h.collect(&mut socket, 5);
    let db = workspace_database(&h);
    let operation = db.try_operation("registered").unwrap();
    let prepared = db
        .prepare_agent(
            &operation,
            "/demo".into(),
            Some("registered".into()),
            None,
            h.root.path().join("driver/tool.toml"),
            json!({"node_name":"registered"}),
        )
        .unwrap();
    let before = db.subscribe_changes().borrow().clone().unwrap();
    assert!(before.agents.is_empty());
    assert!(
        h.collect(&mut socket, 3)
            .iter()
            .all(|v| v["type"] != "workspace.sync")
    );
    std::fs::create_dir_all(db.agent_directory(&prepared).unwrap()).unwrap();
    db.commit_agent(&operation, prepared).unwrap();
    drop(operation);
    let sync = h.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(Ok(Message::Text(text))) = socket.next().await {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "workspace.sync"
                        && value["workspaces"][0]["agents"][0]["node_name"] == "registered"
                    {
                        break value;
                    }
                }
            }
        })
        .await
        .unwrap()
    });
    assert_eq!(sync["workspaces"][0]["agents"][0]["category"], "normal");
    assert!(sync["workspaces"][0]["agents"][0].get("task_id").is_none());
    assert!(
        h.collect(&mut socket, 5)
            .iter()
            .all(|v| v["type"] != "workspace.sync")
    );
}
#[test]
fn archive_unstarted_registration_moves_directory_and_publishes_committed_category() {
    let mut h = Harness::with_real_workspace();
    let record = register_record(&h, "archivable");
    let db = workspace_database(&h);
    let original = db.agent_directory(&record).unwrap();
    std::fs::write(original.join("sentinel"), "preserved").unwrap();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "archive");
    h.request(
        &mut socket,
        "agent.archive",
        "archive",
        json!({"workspace":"/demo","node_name":"archivable"}),
    );
    let response = h.next(&mut socket, |v| v["id"] == "archive");
    assert_eq!(response["success"], true, "{response}");
    assert!(!original.exists());
    h.request(&mut socket, "workspace.req", "archived", json!({}));
    let response = h.next(&mut socket, |v| v["id"] == "archived");
    let rows = response["workspaces"].as_array().unwrap();
    let row = &rows.iter().find(|r| r["address"] == "/demo").unwrap()["agents"][0];
    assert_eq!(row["category"], "archived");
    assert!(row["node_name"].is_null());
    assert_eq!(row["previous_node_name"], "archivable");
    let snapshot = db.subscribe_changes().borrow().clone().unwrap();
    assert_eq!(
        std::fs::read_to_string(
            db.agent_directory(&snapshot.agents[0])
                .unwrap()
                .join("sentinel")
        )
        .unwrap(),
        "preserved"
    );
}

#[test]
fn registered_and_new_starts_dispatch_actual_path_requests_and_duplicate_new_is_rejected() {
    let mut h = Harness::with_real_workspace();
    let record = register_record(&h, "known");
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "actual-start");
    h.request(
        &mut socket,
        "agent.start",
        "existing",
        json!({"mode":"existing","workspace":"/demo","node_name":"known"}),
    );
    h.wait(|h| {
        !h.app
            .world()
            .event_reader::<tool_core::ToolExecuteRequest>()
            .is_empty()
    });
    let request = h
        .app
        .world()
        .event_reader::<tool_core::ToolExecuteRequest>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    let tool_core::ToolExecuteRequest::Path {
        initiator,
        path,
        arguments,
        reply,
    } = request
    else {
        panic!("expected Path");
    };
    assert_eq!(initiator, h.workspace);
    assert_eq!(path, record.tool_path);
    assert_eq!(arguments, record.tool_arguments);
    reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(tool_core::ToolExecuteResponse {
            call_id: "known-task".into(),
            alias: "driver".into(),
            content: "ready".into(),
            failed: false,
            images: vec![],
        }))
        .unwrap();
    assert_eq!(
        h.next(&mut socket, |v| v["id"] == "existing")["success"],
        true
    );
    h.request(&mut socket,"agent.start","duplicate",json!({"mode":"new","workspace":"/demo","tool_path":record.tool_path,"arguments":{"node_name":"known"}}));
    let duplicate = h.next(&mut socket, |v| v["id"] == "duplicate");
    assert_eq!(duplicate["success"], false);
    assert_eq!(
        workspace_database(&h).agent_by_node("known").unwrap().id,
        record.id
    );
    h.request(&mut socket,"agent.start","new",json!({"mode":"new","workspace":"/demo","tool_path":record.tool_path,"arguments":{"node_name":"fresh","alias":"Fresh","path":"src"}}));
    h.wait(|h| {
        !h.app
            .world()
            .event_reader::<tool_core::ToolExecuteRequest>()
            .is_empty()
    });
    let request = h
        .app
        .world()
        .event_reader::<tool_core::ToolExecuteRequest>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    let tool_core::ToolExecuteRequest::Path {
        arguments, reply, ..
    } = request
    else {
        panic!("expected Path");
    };
    assert_eq!(
        arguments,
        json!({"node_name":"fresh","alias":"Fresh","path":"src"})
    );
    assert!(
        workspace_database(&h).agent_by_node("fresh").is_err(),
        "starting Driver must not pre-register Agent"
    );
    reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(tool_core::ToolExecuteResponse {
            call_id: "new-task".into(),
            alias: "driver".into(),
            content: "returned".into(),
            failed: false,
            images: vec![],
        }))
        .unwrap();
    assert_eq!(h.next(&mut socket, |v| v["id"] == "new")["success"], true);
}

#[test]
fn stop_resolves_created_agents_parent_task_id_and_waits_for_real_cleanup() {
    use app_runtime_plugin::WorldEventExt;
    let mut h = Harness::with_real_workspace();
    let tasks = h
        .app
        .world()
        .get_component::<ToolTasks>(h.workspace)
        .unwrap()
        .clone();
    let cleanup = tasks.clone();
    let finish: tool_core::TaskFinish = Arc::new(move |handle| {
        let tasks = cleanup.clone();
        Box::pin(async move {
            handle.cancel();
            handle.finish_children().await?;
            handle.retry_cleanups().await?;
            assert!(tasks.complete_cleanup("creator-task", &handle, None));
            Ok(())
        })
    });
    let (reply, _) = oneshot::channel();
    let (task, _) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "unrelated-call-id".into(),
        "driver".into(),
        reply,
        tasks.update_context(),
        None,
        finish,
    )
    .unwrap();
    tasks.register("creator-task".into(), task.clone()).unwrap();
    let storage_dir = h.root.path().join("child-storage");
    std::fs::create_dir_all(&storage_dir).unwrap();
    let storage = open_storage(
        &storage_dir.join("state.sqlite"),
        &storage_dir.join("setting.toml"),
        "enabled=true",
    )
    .unwrap();
    let (reply, mut response) = oneshot::channel();
    h.app.world().send_event(agent_core::StartAgent {
        workspace: h.workspace,
        parent: h.workspace,
        task_id: "creator-task".into(),
        address: "/demo/child".into(),
        project_root: h.root.path().into(),
        ui_path: h.root.path().join("missing-child-ui"),
        persistence: storage.clone(),
        reply: Arc::new(Mutex::new(Some(reply))),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let handle = loop {
        h.app.tick();
        match response.try_recv() {
            Ok(result) => break result.unwrap(),
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(error) => panic!("{error}"),
        };
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(
        h.app
            .world()
            .get_component::<Agent>(handle.entity())
            .unwrap()
            .task_id(),
        "creator-task"
    );
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "stop-task");
    h.request(
        &mut socket,
        "agent.stop",
        "stop-child",
        json!({"address":"/demo/child"}),
    );
    let stopped = h.next(&mut socket, |v| v["id"] == "stop-child");
    assert_eq!(stopped["success"], true, "{stopped}");
    assert!(!h.app.world().is_alive(handle.entity()));
    assert!(tasks.list().is_empty());
    assert!(task.is_cleanup_complete());
    assert!(!storage.is_open());
    assert!(storage_dir.is_dir());
}

fn resource_query_fixture(h: &mut Harness) {
    h.app
        .world_mut()
        .insert_resource(address::ResourceRoots::new(h.root.path()));
    for (folder, entry) in [
        ("tools/team/coder/dev", "tool.toml"),
        ("tools/team/coder/v2", "tool.toml"),
        ("tools/team/coders/dev", "tool.toml"),
        ("tools/other/coder/dev", "tool.toml"),
        ("agent-drivers/team/driver/dev", "base.lua"),
        ("agent-drivers/team/driver/dev", "tool.toml"),
        ("prompts/team/context/dev", "prompt.toml"),
        ("sandboxes/team/safe/dev", "policy.json"),
    ] {
        let directory = h.root.path().join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(entry), "resource fixture, no execution").unwrap();
    }
}
#[test]
fn resource_queries_filter_exactly_and_return_real_entries_without_activating_tasks() {
    let mut h = Harness::new();
    resource_query_fixture(&mut h);
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "resources");
    let agents = h.app.world().query_with::<Agent>().result();
    h.request(
        &mut socket,
        "resource.req",
        "filtered",
        json!({"type":"tool","scope":"team","name":"coder"}),
    );
    let response = h.next(&mut socket, |v| v["id"] == "filtered");
    assert_eq!(response["type"], "resource.res");
    assert_eq!(response["success"], true);
    let rows = response["resources"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["address"], "tool:team/coder:dev");
    assert_eq!(rows[1]["tag"], "v2");
    assert_eq!(rows[0]["path"], rows[0]["tool_path"]);
    assert!(std::path::Path::new(rows[0]["path"].as_str().unwrap()).is_absolute());
    h.request(
        &mut socket,
        "resource.req",
        "driver",
        json!({"type":"driver","scope":"team"}),
    );
    let driver = h.next(&mut socket, |v| v["id"] == "driver");
    assert!(
        driver["resources"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("base.lua")
    );
    assert!(
        driver["resources"][0]["tool_path"]
            .as_str()
            .unwrap()
            .ends_with("tool.toml")
    );
    h.request(
        &mut socket,
        "resource.req",
        "empty",
        json!({"name":"missing"}),
    );
    assert_eq!(
        h.next(&mut socket, |v| v["id"] == "empty")["resources"],
        json!([])
    );
    std::fs::write(
        h.root.path().join("tools/team/coder/v2/tool.toml"),
        "still opaque",
    )
    .unwrap();
    let removed = h.root.path().join("tools/team/coder/dev/tool.toml");
    assert!(removed.starts_with(h.root.path()));
    std::fs::remove_file(removed).unwrap();
    h.request(
        &mut socket,
        "resource.req",
        "fresh",
        json!({"type":"tool","scope":"team","name":"coder"}),
    );
    assert_eq!(
        h.next(&mut socket, |v| v["id"] == "fresh")["resources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(h.app.world().query_with::<Agent>().result(), agents);
    assert!(
        h.app
            .world()
            .get_component::<ToolTasks>(h.agent)
            .unwrap()
            .list()
            .is_empty()
    );
}
#[test]
fn resource_scan_replies_on_original_oneshot_without_extra_world_frames() {
    let mut h = Harness::new();
    resource_query_fixture(&mut h);
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "resources-no-frame");
    h.request(
        &mut socket,
        "resource.req",
        "scan-no-frame",
        json!({"type":"prompt","scope":"team"}),
    );
    h.ingress_barrier(&mut socket);
    h.app.tick();
    h.app.tick();
    let value = h.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(Ok(Message::Text(text))) = socket.next().await {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["id"] == "scan-no-frame" {
                        break value;
                    }
                }
            }
        })
        .await
        .unwrap()
    });
    assert_eq!(value["success"], true);
    assert_eq!(value["resources"][0]["address"], "prompt:team/context:dev");
    assert!(value["resources"][0]["tool_path"].is_null());
}
#[test]
fn resource_query_requires_registration_and_read_failure_is_not_an_empty_success() {
    let mut h = Harness::new();
    resource_query_fixture(&mut h);
    let mut socket = h.connect();
    h.request(
        &mut socket,
        "resource.req",
        "unregistered-resources",
        json!({}),
    );
    let rejected = h.next(&mut socket, |v| v["id"] == "unregistered-resources");
    assert_eq!(rejected["success"], false);
    assert_eq!(rejected["error"]["code"], "Unregistered");
    assert!(rejected.get("resources").is_none());
    h.register(&mut socket, "webui", "resources-error");
    let bad = h.root.path().join("bad-resource-root");
    std::fs::write(&bad, "not a directory").unwrap();
    h.app
        .world_mut()
        .get_resource_mut::<address::ResourceRoots>()
        .unwrap()
        .tool = bad;
    h.request(
        &mut socket,
        "resource.req",
        "read-error",
        json!({"type":"tool"}),
    );
    let failed = h.next(&mut socket, |v| v["id"] == "read-error");
    assert_eq!(failed["success"], false);
    assert_eq!(failed["error"]["code"], "ResourceReadFailed");
    assert!(failed.get("resources").is_none());
}

#[cfg(unix)]
#[test]
fn resource_res_preserves_internal_alias_paths_and_component_sort_order() {
    use std::os::unix::fs::symlink;
    let mut h = Harness::new();
    resource_query_fixture(&mut h);
    let folder = h.root.path().join("tools/prefix/a/dev");
    std::fs::create_dir_all(&folder).unwrap();
    let real = folder.join("actual-manifest.data");
    std::fs::write(&real, "opaque").unwrap();
    symlink(&real, folder.join("tool.toml")).unwrap();
    let other = h.root.path().join("tools/prefix/a-/dev");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("tool.toml"), "opaque").unwrap();
    let mut socket = h.connect();
    h.register(&mut socket, "webui", "resources-alias");
    h.request(
        &mut socket,
        "resource.req",
        "alias-paths",
        json!({"type":"tool","scope":"prefix"}),
    );
    let reply = h.next(&mut socket, |v| v["id"] == "alias-paths");
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(reply["resources"][0]["name"], "a");
    assert_eq!(reply["resources"][1]["name"], "a-");
    assert_eq!(
        reply["resources"][0]["path"],
        real.canonicalize().unwrap().to_str().unwrap()
    );
}
