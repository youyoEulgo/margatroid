use address::Address;
use agent_core::{Agent, AgentState, RestartAgent, StartAgent, WorldAgentExt};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use axum_plugin::{AxumHandle, AxumPlugin};
use client_core::{Client, ConnectionId, WorldClientExt};
use client_plugin::ClientPlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use config_core::{LogLevel, MargatroidConfig, OutboundTarget};
use core_plugin::{App, Entity, World};
use dto_plugin::DtoPlugin;
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
    StartWorkspace, Workspace, WorkspaceDefinitions, WorkspaceOperationOutput, WorkspaceState,
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
        let definitions_root = root.path().join("workspaces");
        std::fs::create_dir_all(definitions_root.join("demo")).unwrap();
        std::fs::write(definitions_root.join("demo/workspace.yaml"), format!(
            "name: demo\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/new:latest\n    home_root: {}\n",
            root.path().display(), root.path().join("new-home").display()
        )).unwrap();
        app.world_mut()
            .insert_resource(WorkspaceDefinitions::open(&definitions_root).unwrap());
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                definition: None,
                name: "demo".into(),
                definition_path: definitions_root.join("demo/workspace.yaml"),
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
                &root.path().join("setting.toml"),
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
            json!({"address": "agent:demo/coder", "command": command, "arguments": arguments}),
        );
    }
    fn target(&self) -> Value {
        json!({"address": "agent:demo/coder"})
    }
    fn definition(&self) -> std::path::PathBuf {
        self.root.path().join("workspaces/demo/workspace.yaml")
    }
    fn add_definition(&self, directory: &str, name: &str) -> std::path::PathBuf {
        let parent = self.root.path().join("workspaces").join(directory);
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join("workspace.yaml");
        std::fs::write(
            &path,
            format!(
                "name: {name}\nproject_root: {}\nmembers: []\n",
                self.root.path().display()
            ),
        )
        .unwrap();
        path
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
            .starts_with("client:webui/browser:")
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
fn agent_start_and_restart_reload_members_and_only_confirm_operation_results() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(
        &mut socket,
        "agent.start",
        "start",
        json!({"workspace":"workspace:local/demo","name":"coder"}),
    );
    harness.wait(|harness| !harness.starts.lock().unwrap().is_empty());
    assert!(harness.restarts.lock().unwrap().is_empty());
    let request = harness.starts.lock().unwrap()[0].clone();
    assert!(
        request
            .parameters
            .database_path
            .ends_with("workspaces/demo/agents/coder/state.sqlite")
    );
    assert!(
        request
            .parameters
            .settings_path
            .ends_with("workspaces/demo/agents/coder/setting.toml")
    );
    assert_eq!(request.parameters.driver, "driver:local/new:latest");
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(harness.agent))
        .unwrap();
    let result = harness.next(&mut socket, |value| value["id"] == "start");
    assert_eq!(
        result,
        json!({"type":"agent.start.result","id":"start","success":true})
    );
    std::fs::write(harness.definition(), format!(
        "name: demo\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/refreshed:latest\n    home_root: {}\n",
        harness.root.path().display(), harness.root.path().join("fresh-home").display()
    )).unwrap();
    harness.request(&mut socket, "agent.restart", "restart", harness.target());
    harness.wait(|harness| !harness.restarts.lock().unwrap().is_empty());
    let request = harness.restarts.lock().unwrap()[0].clone();
    assert_eq!(request.parameters.driver, "driver:local/refreshed:latest");
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
    assert_eq!(
        reply,
        json!({"type":"agent.restart.result","id":"restart","success":true})
    );
    harness.request(&mut socket, "agent.req", "live", json!({}));
    let states = harness.next(&mut socket, |value| value["id"] == "live");
    assert!(states["agents"][0].get("instance").is_none());
    assert!(states["agents"][0].get("name").is_none());
}

#[test]
fn workspace_queries_show_scanned_definitions_and_stop_uses_definition_path() {
    let mut harness = Harness::new();
    let cached = harness.add_definition("inactive", "inactive");
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    harness.request(&mut socket, "workspace.reload", "reload", json!({}));
    let reload = harness.next(&mut socket, |value| value["id"] == "reload");
    assert_eq!(
        reload,
        json!({"type":"workspace.reload.result","id":"reload","success":true})
    );
    harness.request(&mut socket, "workspace.req", "definitions", json!({}));
    let definitions = harness.next(&mut socket, |value| value["id"] == "definitions");
    assert_eq!(definitions["type"], "workspace.res");
    assert_eq!(definitions["success"], true);
    let scanned = definitions["workspaces"].as_array().unwrap();
    let inactive = scanned
        .iter()
        .find(|entry| entry["path"] == cached.to_string_lossy().as_ref())
        .unwrap();
    assert_eq!(inactive["definition"]["name"], "inactive");
    assert_eq!(inactive["status"], "inactive");
    assert!(inactive["address"].is_null());
    assert!(inactive.get("member_failures").is_none());
    assert!(harness.workspace_starts.lock().unwrap().is_empty());
    harness.request(
        &mut socket,
        "workspace.start",
        "start-path",
        json!({"path": cached}),
    );
    harness.wait(|harness| !harness.workspace_starts.lock().unwrap().is_empty());
    let event = harness.workspace_starts.lock().unwrap()[0].clone();
    assert_eq!(event.path, cached);
    event
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(WorkspaceOperationOutput {
            workspace: harness.workspace,
            members: vec![harness.agent],
            member_errors: vec![],
        }))
        .unwrap();
    assert_eq!(
        harness.next(&mut socket, |value| value["id"] == "start-path"),
        json!({"type":"workspace.start.result","id":"start-path","success":true})
    );
    harness.request(
        &mut socket,
        "workspace.stop",
        "stop",
        json!({"path":harness.definition()}),
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
    stop.reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(WorkspaceOperationOutput {
            workspace: stop.workspace,
            members: vec![],
            member_errors: vec![],
        }))
        .unwrap();

    let reply = harness.next(&mut socket, |value| value["id"] == "stop");
    assert_eq!(
        reply,
        json!({"type":"workspace.stop.result","id":"stop","success":true})
    );
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
        .get_component_mut::<Agent>(harness.agent)
        .unwrap()
        .state = AgentState::Working;
    let state = harness.next(&mut socket, |value| {
        value["type"] == "agent.sync" && value["agents"][0]["status"] == "working"
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
        .get_component_mut::<Agent>(harness.agent)
        .unwrap()
        .turn = Some("changed".into());
    let turn = harness.next(&mut browser, |value| {
        value["type"] == "agent.turn.sync" && value["turn"] == "changed"
    });
    assert_eq!(turn["address"], "agent:demo/coder:latest");
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
    harness.request(
        &mut socket,
        "agent.turn.abort",
        "abort",
        json!({"address": "agent:demo/coder", "turn": turn}),
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
    harness.request(
        &mut socket,
        "agent.turn.abort",
        "stale",
        json!({"address": "agent:demo/coder", "turn": format!("{turn}-stale")}),
    );
    harness.ingress_barrier(&mut socket);
    harness.app.tick();
    vm(harness.app.world_mut(), harness.agent, "driver-new");
    let reply = harness.next(&mut socket, |value| value["id"] == "stale");
    assert_eq!(reply["success"], false);
    assert_eq!(reply["error"]["code"], "StateRejected");
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
    let first_path = harness.add_definition("first", "first");
    let second_path = harness.add_definition("second", "second");
    for (socket, path) in [(&mut first, &first_path), (&mut second, &second_path)] {
        harness.request(socket, "workspace.start", "same-id", json!({"path": path}));
    }
    harness.wait(|harness| harness.workspace_starts.lock().unwrap().len() == 2);
    let requests = harness.workspace_starts.lock().unwrap().clone();
    assert_ne!(requests[0].id, requests[1].id);
    assert!(requests.iter().all(|request| request.id != "same-id"));
    let response = requests
        .iter()
        .find(|request| request.path == first_path)
        .unwrap()
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap();
    let _ = response.send(Ok(WorkspaceOperationOutput {
        workspace: harness.workspace,
        members: vec![],
        member_errors: vec![],
    }));
    assert_eq!(
        harness.next(&mut first, |value| value["type"]
            == "workspace.start.result")["id"],
        "same-id"
    );
    assert!(
        harness
            .collect(&mut second, 5)
            .iter()
            .all(|value| value["type"] != "workspace.start.result")
    );
    harness.runtime.block_on(second.close(None)).unwrap();
    drop(second);
    harness.wait(|harness| harness.app.world().query_with::<Client>().result().len() == 1);
    let response = requests
        .iter()
        .find(|request| request.path == second_path)
        .unwrap()
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap();
    let _ = response.send(Ok(WorkspaceOperationOutput {
        workspace: harness.workspace,
        members: vec![],
        member_errors: vec![],
    }));
    let mut replacement = harness.connect();
    harness.register(&mut replacement, "webui", "second");
    assert!(
        harness
            .collect(&mut replacement, 8)
            .iter()
            .all(|value| value["type"] != "workspace.start.result")
    );
    assert!(harness.app.world().client_by_connection(first_id).is_some());
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
fn agent_projection_preserves_all_tool_attributes_and_excludes_other_domains() {
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
    assert!(agent.get("sandboxes").is_none() && agent.get("sandbox").is_none());
    assert!(
        agent.get("turn").is_none()
            && agent.get("tasks").is_none()
            && agent.get("histories").is_none()
    );
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
fn original_oneshot_reaches_websocket_without_another_world_frame() {
    let mut harness = Harness::new();
    let mut socket = harness.connect();
    harness.register(&mut socket, "webui", "browser");
    let path = harness.add_definition("direct", "direct");
    harness.request(
        &mut socket,
        "workspace.start",
        "direct-reply",
        json!({"path":path}),
    );
    harness.wait(|h| !h.workspace_starts.lock().unwrap().is_empty());
    let request = harness.workspace_starts.lock().unwrap()[0].clone();
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(WorkspaceOperationOutput {
            workspace: harness.workspace,
            members: vec![],
            member_errors: vec![],
        }))
        .unwrap();
    let result = harness.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["id"] == "direct-reply" {
                    break value;
                }
            }
        })
        .await
        .unwrap()
    });
    assert_eq!(
        result,
        json!({"type":"workspace.start.result","id":"direct-reply","success":true})
    );
}
