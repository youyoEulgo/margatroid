use address::Address;
use agent_core::{Agent, AgentState, RestartAgent, StartAgent, WorldAgentExt};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use axum_plugin::{AxumHandle, AxumPlugin};
use client_core::{Client, ConnectionId, WorldClientExt};
use client_plugin::ClientPlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use config_core::{LogLevel, MargatroidConfig, OutboundTarget};
use core_plugin::{App, Entity, World};
use dto_plugin::{DtoPlugin, WebSocketMessageSend};
use futures_util::{SinkExt, StreamExt};
use inference_core::{
    ActiveInference, InferenceHandle, InferenceRequest, InferenceResponse, InferenceStopReason,
};
use lua_runtime_core::{DriverLuaVm, DriverLuaVmInstance, DriverWakeSlot};
use mcl_core::MclQueue;
use mcl_plugin::MclPlugin;
use serde_json::{Value, json};
use std::collections::BTreeSet;
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
    StartWorkspace, StartWorkspaceResult, Workspace, WorkspaceDefinition, WorkspaceDefinitions,
    WorkspaceOperationOutput, WorkspaceState,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Harness {
    app: App,
    runtime: tokio::runtime::Runtime,
    server: SocketAddr,
    root: tempfile::TempDir,
    workspace: Entity,
    agent: Entity,
    starts: Arc<Mutex<Vec<StartAgent>>>,
    restarts: Arc<Mutex<Vec<RestartAgent>>>,
    workspace_starts: Arc<Mutex<Vec<StartWorkspace>>>,
    inference: Arc<Mutex<Vec<InferenceRequest>>>,
}

fn vm(world: &mut World, owner: Entity, id: &str) {
    let (ready, _receiver) = oneshot::channel();
    let (_sender, ended) = oneshot::channel();
    world.insert_component(
        owner,
        DriverLuaVm {
            instance: DriverLuaVmInstance {
                owner,
                start_id: id.into(),
            },
            wake: DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(Some(ready))),
            ended: Some(ended),
            cancel: CancellationToken::new(),
        },
    );
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
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
            MargatroidConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                LogLevel::Info,
                None,
                vec![
                    OutboundTarget::Type("cli".into()),
                    OutboundTarget::Name("terminal".into()),
                ],
                vec![
                    OutboundTarget::Type("webui".into()),
                    OutboundTarget::Name("browser".into()),
                ],
            )
            .unwrap(),
        );
        app.world_mut()
            .insert_resource(WorkspaceDefinitions::default());
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                name: "demo".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().into(),
                state: WorkspaceState::Running,
                agents: BTreeSet::new(),
            },
        );
        app.world_mut()
            .insert_component(workspace, Address::new("workspace:local/demo").unwrap());
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            Agent {
                name: "coder".into(),
                workspace,
                state: AgentState::Idle,
                turn: None,
            },
        );
        app.world_mut()
            .insert_component(agent, Address::new("agent:demo/coder").unwrap());
        app.world_mut().insert_component(
            agent,
            open_storage(
                &root.path().join("state.sqlite"),
                &root.path().join("settings.toml"),
                "enabled = true\n",
            )
            .unwrap(),
        );
        app.world_mut().insert_component(agent, MclQueue::default());
        app.world_mut()
            .insert_component(agent, ActiveInference::default());
        app.world_mut().insert_component(agent, ToolTasks::new());
        vm(app.world_mut(), agent, "driver-first");
        app.world_mut()
            .get_component_mut::<Workspace>(workspace)
            .unwrap()
            .agents
            .insert(agent);
        let starts = Arc::new(Mutex::new(Vec::new()));
        let recorded = starts.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            for request in world
                .event_reader::<StartAgent>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                recorded.lock().unwrap().push(request);
            }
        });
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let recorded = restarts.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            for request in world
                .event_reader::<RestartAgent>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                recorded.lock().unwrap().push(request);
            }
        });
        let workspace_starts = Arc::new(Mutex::new(Vec::new()));
        let recorded = workspace_starts.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            recorded
                .lock()
                .unwrap()
                .extend(world.event_reader::<StartWorkspace>().into_iter().cloned());
        });
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
        app.add_plugin(DtoPlugin::new());
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
            starts,
            restarts,
            workspace_starts,
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
        let address = registration["client"]["resource_id"].as_str().unwrap();
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
        self.next(socket, |value| value["type"] == "connection.registered")
    }
    fn request(&self, socket: &mut Socket, kind: &str, id: &str, message: Value) {
        self.send(socket, json!({"type": kind, "id": id, "message": message}));
    }
    fn command(&self, socket: &mut Socket, id: &str, command: &str, arguments: Value) {
        self.request(socket, "mcl.command", id, json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder", "command": command, "arguments": arguments}));
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
    assert!(
        registered["client"]["resource_id"]
            .as_str()
            .unwrap()
            .starts_with("client:webui/browser:")
    );
    let (client, _) = harness
        .app
        .world()
        .client_by_connection(connection)
        .unwrap();
    harness.command(&mut socket, "network-id", "CONFIG GET", json!([]));
    let reply = harness.next(&mut socket, |value| value["type"] == "mcl.command_result");
    assert_eq!(reply["id"], "network-id");
    assert_eq!(reply["source"], registered["client"]["resource_id"]);
    assert_eq!(reply["target"], "agent:demo/coder:latest");
    assert_eq!(reply["result"]["Ok"]["Config"]["enabled"], true);
    assert_eq!(
        harness
            .app
            .world()
            .client_by_connection(connection)
            .unwrap()
            .0,
        client
    );
    harness.request(
        &mut socket,
        "history.query",
        "history",
        json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder"}),
    );
    let history = harness.next(&mut socket, |value| value["type"] == "history.result");
    assert!(
        history["history"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["kind"] == "mcl"
                && entry["payload"]["id"].as_str().unwrap().starts_with("dto:")
                && entry["payload"]["id"] != "network-id"
                && entry["source"] == registered["client"]["resource_id"]
                && entry.get("key").is_none())
    );
}

#[test]
fn agent_start_and_restart_follow_request_kind_and_return_live_addresses() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    let message = json!({"workspace": "workspace:local/demo", "name": "coder", "parameters": {"driver": "driver:local/new", "home_root": "new-home"}});
    harness.request(&mut socket, "agent.start", "start", message.clone());
    harness.wait(|harness| !harness.starts.lock().unwrap().is_empty());
    assert!(harness.restarts.lock().unwrap().is_empty());
    let request = harness.starts.lock().unwrap()[0].clone();
    assert!(
        request
            .parameters
            .database_path
            .ends_with("new-home/state.sqlite")
    );
    assert_eq!(request.parameters.driver, "driver:local/new");
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(harness.agent))
        .unwrap();
    assert_eq!(
        harness.next(&mut socket, |value| value["id"] == "start")["type"],
        "agent.started"
    );
    harness.request(&mut socket, "agent.restart", "restart", message);
    harness.wait(|harness| !harness.restarts.lock().unwrap().is_empty());
    let request = harness.restarts.lock().unwrap()[0].clone();
    vm(harness.app.world_mut(), harness.agent, "driver-restarted");
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(harness.agent))
        .unwrap();
    let reply = harness.next(&mut socket, |value| value["id"] == "restart");
    assert_eq!(reply["type"], "agent.restarted");
    assert_eq!(reply["agent"], "agent:demo/coder:latest");
}

#[test]
fn workspace_paths_and_definition_cache_are_direct_and_stop_preserves_public_reference() {
    let mut harness = Harness::new();
    harness
        .app
        .world()
        .get_resource::<WorkspaceDefinitions>()
        .unwrap()
        .entries
        .write()
        .unwrap()
        .insert(
            "/not-on-disk/example.yaml".into(),
            WorkspaceDefinition {
                name: "cached".into(),
                project_root: "/cached".into(),
                members: vec![],
            },
        );
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(
        &mut socket,
        "workspace.definitions.query",
        "definitions",
        json!({}),
    );
    let definitions = harness.next(&mut socket, |value| {
        value["type"] == "workspace.definitions"
    });
    assert_eq!(
        definitions["definitions"][0]["path"],
        "/not-on-disk/example.yaml"
    );
    assert!(harness.workspace_starts.lock().unwrap().is_empty());
    harness.request(
        &mut socket,
        "workspace.start",
        "start-path",
        json!({"path": "/requested/fresh.yaml"}),
    );
    harness.wait(|harness| !harness.workspace_starts.lock().unwrap().is_empty());
    let event = harness.workspace_starts.lock().unwrap()[0].clone();
    assert_eq!(
        event.path,
        std::path::PathBuf::from("/requested/fresh.yaml")
    );
    harness.app.world().send_event(StartWorkspaceResult {
        id: event.id,
        result: Ok(WorkspaceOperationOutput {
            workspace: harness.workspace,
            members: vec![harness.agent],
            member_errors: vec![],
        }),
    });
    assert_eq!(
        harness.next(&mut socket, |value| value["id"] == "start-path")["type"],
        "workspace.started"
    );
    harness.request(
        &mut socket,
        "workspace.stop",
        "stop",
        json!({"workspace": "workspace:local/demo"}),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let stop = loop {
        harness.app.tick();
        if let Some(event) = harness
            .app
            .world()
            .event_reader::<workspace_core::StopWorkspace>()
            .into_iter()
            .next()
        {
            break event.clone();
        }
        assert!(Instant::now() < deadline);
    };
    harness.app.world_mut().despawn(harness.workspace);
    harness
        .app
        .world()
        .send_event(workspace_core::StopWorkspaceResult {
            id: stop.id,
            result: Ok(WorkspaceOperationOutput {
                workspace: stop.workspace,
                members: vec![],
                member_errors: vec![],
            }),
        });
    let reply = harness.next(&mut socket, |value| value["id"] == "stop");
    assert_eq!(reply["workspace"]["id"], "workspace:local/demo:latest");
    assert_eq!(reply["workspace"]["name"], "demo");
}

#[test]
fn persisted_history_sequences_timestamps_and_state_histories_are_not_reinvented() {
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
        "history.query",
        "persisted",
        json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder"}),
    );
    let history = harness.next(&mut socket, |value| value["type"] == "history.result");
    assert_eq!(history["history"]["messages"][0]["sequence"], 2);
    assert_eq!(history["history"]["messages"][0]["type"], "user");
    assert!(history["history"]["messages"][0].get("source").is_none());
    assert!(history["history"]["messages"][0].get("key").is_none());
    assert_eq!(history["history"]["messages"][0]["created_at_ms"], 5678);
    harness.request(&mut socket, "state.query", "state", json!({}));
    let state = harness.next(&mut socket, |value| {
        value["type"] == "state.sync"
            && value["state"]["histories"]
                .as_array()
                .is_some_and(|histories| !histories.is_empty())
    });
    assert_eq!(state["state"]["histories"][0]["messages"][0]["sequence"], 2);
    harness
        .app
        .world_mut()
        .get_component_mut::<Agent>(harness.agent)
        .unwrap()
        .state = AgentState::Working;
    let state = harness.next(&mut socket, |value| {
        value["type"] == "state.sync" && value["state"]["agents"][0]["status"] == "working"
    });
    assert_eq!(state["state"]["histories"][0]["messages"][0]["sequence"], 2);
    connection.execute("DROP TABLE agent_messages", []).unwrap();
    harness.request(
        &mut socket,
        "history.query",
        "broken",
        json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder"}),
    );
    let failure = harness.next(&mut socket, |value| value["id"] == "broken");
    assert_eq!(failure["type"], "request.failed");
    assert_eq!(failure["error"]["kind"], "HistoryReadFailed");
    assert!(!failure.to_string().contains("agent_messages"));
}

#[test]
fn configured_state_and_logs_deduplicate_targets_and_never_leak_to_other_clients() {
    let mut harness = Harness::new();
    let mut browser = harness.connect();
    harness.register(&mut browser, "webui", "browser");
    harness.next(&mut browser, |value| value["type"] == "state.sync");
    assert!(
        harness
            .collect(&mut browser, 15)
            .iter()
            .all(|value| value["type"] != "state.sync")
    );
    let mut terminal = harness.connect();
    harness.register(&mut terminal, "cli", "terminal");
    assert!(
        harness
            .collect(&mut terminal, 10)
            .iter()
            .all(|value| value["type"] != "state.sync")
    );
    harness
        .app
        .world_mut()
        .get_component_mut::<Agent>(harness.agent)
        .unwrap()
        .turn = Some("changed".into());
    harness.next(&mut browser, |value| {
        value["type"] == "state.sync" && value["state"]["agents"][0]["turn"] == "changed"
    });
    assert!(
        harness
            .collect(&mut browser, 10)
            .iter()
            .all(|value| value["type"] != "state.sync")
    );
    assert!(
        harness
            .collect(&mut terminal, 10)
            .iter()
            .all(|value| value["type"] != "state.sync")
    );
    harness.app.world().send_event(WebSocketMessageSend { target: OutboundTarget::Broadcast, message: json!({"type": "log", "record": {"timestamp_millis": 1, "level": "INFO", "target": "test", "message": "only-terminal", "fields": [], "spans": []}}) });
    assert_eq!(
        harness.next(&mut terminal, |value| value["type"] == "log")["record"]["message"],
        "only-terminal"
    );
    assert!(
        harness
            .collect(&mut terminal, 10)
            .iter()
            .all(|value| value["type"] != "log")
    );
    assert!(
        harness
            .collect(&mut browser, 10)
            .iter()
            .all(|value| value["type"] != "log")
    );
    let mut second = harness.connect();
    harness.register(&mut second, "webui", "second-browser");
    harness.next(&mut second, |value| value["type"] == "state.sync");
    assert!(
        harness
            .collect(&mut browser, 10)
            .iter()
            .all(|value| value["type"] != "state.sync")
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
            turn: turn.clone(),
            cancel: cancel.clone(),
            stop_stream: Arc::new(move || {
                assert!(!observed.is_cancelled());
                marked.store(true, std::sync::atomic::Ordering::Release);
            }),
            task: Default::default(),
        };
        inference.attach_task(handle).unwrap();
        inference
    });
    harness.request(&mut socket, "agent.turn.abort", "abort", json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder", "agent_instance": "driver-first", "turn_id": turn}));
    harness.ingress_barrier(&mut socket);
    harness.app.tick();
    assert!(!cancel.is_cancelled());
    assert!(
        harness
            .app
            .world()
            .get_component::<Agent>(harness.agent)
            .unwrap()
            .turn
            .is_some()
    );
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
    assert_eq!(reply["result"], json!({"Ok": null}));
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
        .turn
        .clone();
    assert!(next_turn.is_none());
    harness
        .app
        .world_mut()
        .get_component_mut::<Agent>(harness.agent)
        .unwrap()
        .state = AgentState::Idle;
    let turn = harness
        .app
        .world_mut()
        .agent_start_turn(harness.agent)
        .unwrap();
    harness.request(&mut socket, "agent.turn.abort", "stale", json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder", "agent_instance": "driver-first", "turn_id": turn}));
    harness.ingress_barrier(&mut socket);
    harness.app.tick();
    vm(harness.app.world_mut(), harness.agent, "driver-new");
    let reply = harness.next(&mut socket, |value| value["id"] == "stale");
    assert_eq!(reply["result"]["Err"]["kind"], "InstanceMismatch");
    assert!(
        harness
            .app
            .world()
            .get_component::<Agent>(harness.agent)
            .unwrap()
            .turn
            .is_some()
    );
}

#[test]
fn same_network_id_on_two_connections_and_disconnected_late_results_stay_isolated() {
    let mut harness = Harness::new();
    let mut first = harness.connect();
    let registered = harness.register(&mut first, "webui", "first");
    let first_id = harness.registered_connection(&registered);
    let mut second = harness.connect();
    harness.register(&mut second, "webui", "second");
    for (socket, path) in [(&mut first, "/first.yaml"), (&mut second, "/second.yaml")] {
        harness.request(socket, "workspace.start", "same-id", json!({"path": path}));
    }
    harness.wait(|harness| harness.workspace_starts.lock().unwrap().len() == 2);
    let requests = harness.workspace_starts.lock().unwrap().clone();
    assert_ne!(requests[0].id, requests[1].id);
    assert!(requests.iter().all(|request| request.id != "same-id"));
    harness.app.world().send_event(StartWorkspaceResult {
        id: requests
            .iter()
            .find(|request| request.path == std::path::Path::new("/first.yaml"))
            .unwrap()
            .id
            .clone(),
        result: Ok(WorkspaceOperationOutput {
            workspace: harness.workspace,
            members: vec![],
            member_errors: vec![],
        }),
    });
    assert_eq!(
        harness.next(&mut first, |value| value["type"] == "workspace.started")["id"],
        "same-id"
    );
    assert!(
        harness
            .collect(&mut second, 5)
            .iter()
            .all(|value| value["type"] != "workspace.started")
    );
    harness.runtime.block_on(second.close(None)).unwrap();
    drop(second);
    harness.wait(|harness| harness.app.world().query_with::<Client>().result().len() == 1);
    harness.app.world().send_event(StartWorkspaceResult {
        id: requests
            .iter()
            .find(|request| request.path == std::path::Path::new("/second.yaml"))
            .unwrap()
            .id
            .clone(),
        result: Ok(WorkspaceOperationOutput {
            workspace: harness.workspace,
            members: vec![],
            member_errors: vec![],
        }),
    });
    let mut replacement = harness.connect();
    harness.register(&mut replacement, "webui", "second");
    assert!(
        harness
            .collect(&mut replacement, 8)
            .iter()
            .all(|value| value["type"] != "workspace.started")
    );
    assert!(harness.app.world().client_by_connection(first_id).is_some());
}

#[test]
fn oversized_history_and_state_queries_return_correlated_failures() {
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
        "history.query",
        "large-history",
        json!({"workspace": "workspace:local/demo", "agent": "agent:demo/coder"}),
    );
    let reply = harness.next(&mut socket, |value| value["id"] == "large-history");
    assert_eq!(reply["type"], "request.failed");
    assert_eq!(reply["error"]["kind"], "ResponseTooLarge");
    harness.request(&mut socket, "state.query", "large-state", json!({}));
    let reply = harness.next(&mut socket, |value| value["id"] == "large-state");
    assert_eq!(reply["type"], "request.failed");
    assert_eq!(reply["error"]["kind"], "ResponseTooLarge");
}

#[test]
fn stale_vm_mcl_results_do_not_bind_to_new_instance_and_invalid_input_never_echoes_secrets() {
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
        inference_core::AgentInferenceConfig {
            parameters: inference_core::InferenceParameters {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
        },
    );
    harness.command(
        &mut socket,
        "pending",
        "EFFECT inference ?",
        json!([[{"type": "user", "content": "question"}]]),
    );
    harness.wait(|harness| !harness.inference.lock().unwrap().is_empty());
    vm(harness.app.world_mut(), harness.agent, "new-instance");
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
            replay: None,
            usage: None,
            stop_reason: InferenceStopReason::Completed,
        }))
        .unwrap();
    let response = harness.next(&mut socket, |value| value["id"] == "pending");
    assert_eq!(response["type"], "request.failed");
    assert_eq!(response["error"]["kind"], "TargetChanged");
    assert!(!response.to_string().contains("old-private-result"));
    let secret = "PRIVATE-RAW-REQUEST";
    harness.request(
        &mut socket,
        "history.query",
        "invalid",
        json!({"workspace": {}, "private": secret}),
    );
    let failure = harness.next(&mut socket, |value| value["id"] == "invalid");
    assert!(
        failure["error"]["message"]
            .as_str()
            .unwrap()
            .contains("message.agent")
    );
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
fn resource_panel_snapshot_advertises_imported_aliases_without_file_paths() {
    use sandbox_resource_core::{Sandboxes, SelectedSandbox};
    use tool_resource_core::{ToolEntry, Tools};
    let mut harness = Harness::new();
    let mut tools = Tools::default();
    tools
        .insert(
            "bash".into(),
            ToolEntry {
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
                path: harness.root.path().join("private/read-file/tool.toml"),
                description: "read file".into(),
                input_schema: json!({"type":"object"}),
                owner_task_id: None,
                binding: None,
            },
        )
        .unwrap();
    let mut sandboxes = Sandboxes::default();
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
    harness.app.world_mut().insert_component(
        harness.agent,
        SelectedSandbox {
            alias: Some("read_only".into()),
        },
    );
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "resource-panel");
    harness.request(&mut socket, "state.query", "resources", json!({}));
    let state = harness.next(&mut socket, |value| {
        value["type"] == "state.sync"
            && value["state"]["agents"][0]["tools"] == json!(["bash", "read_file"])
    });
    let agent = &state["state"]["agents"][0];
    assert_eq!(agent["sandboxes"], json!(["read_only"]));
    assert_eq!(agent["sandbox"], "read_only");
    assert!(!agent.to_string().contains("private/"));
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
