use agent_core::{Agent, AgentState};
use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity};
use lua_runtime_core::DriverLuaVm;
use mcl_core::{MclCommand, MclInstruction, MclValue};
use serde_json::json;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage_core::{AgentStorage, StorageOperation, StorageValue};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{
    StartWorkspace, StartWorkspaceResult, StopWorkspace, StopWorkspaceResult, Workspace,
};

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn get_static(address: std::net::SocketAddr, path: &str) -> (String, Vec<u8>) {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap()
        + 4;
    (
        String::from_utf8(response[..split].to_vec()).unwrap(),
        response[split..].to_vec(),
    )
}

fn command(
    app: &mut App,
    agent: Entity,
    text: &str,
    arguments: Vec<serde_json::Value>,
) -> MclValue {
    let instance = app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .instance
        .start_id
        .clone();
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("test:{text}"),
        source: agent,
        source_address: app
            .world()
            .get_component::<address::Address>(agent)
            .unwrap()
            .as_str()
            .to_owned(),
        source_instance: Some(instance),
        target: agent,
        instruction: MclInstruction {
            text: text.into(),
            arguments,
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Ok(value) = rx.try_recv() {
            return value.unwrap();
        }
        assert!(Instant::now() < deadline, "MCL did not finish: {text}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn bundled_driver_runs_real_mcl_storage_and_http_then_stops_its_workspace() {
    let root = tempfile::tempdir().unwrap();
    let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources");
    copy_tree(&resources, root.path());
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let directory = root.path().join("workspaces/demo");
    std::fs::create_dir_all(&directory).unwrap();
    let definition = directory.join("workspace.yaml");
    std::fs::write(&definition, format!("name: demo\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/coder:latest\n    home_root: {}\n", project.display(), root.path().display())).unwrap();
    std::fs::write(root.path().join("config.toml"), "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:unmatched']\nbackend_state=['type:unmatched']\n").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    let provider = std::thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 8192];
        let header_end = loop {
            let read = connection.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
            if let Some(index) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() < header_end + length {
            let read = connection.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
        }
        request_tx
            .send(
                serde_json::from_slice::<serde_json::Value>(
                    &request[header_end..header_end + length],
                )
                .unwrap(),
            )
            .unwrap();
        let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Finished through the new stack.\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        write!(connection, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        connection.flush().unwrap();
    });
    std::fs::write(root.path().join("models.toml"), format!("[[models]]\nid='openrouter/deepseek-v4.1-flash'\nmodel='offline-model'\nbase_url='http://{endpoint}/v1'\napi_key='local-test'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n")).unwrap();
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().to_owned(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    app.world().emit_event(StartWorkspace {
        id: "start".into(),
        path: definition,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let workspace = loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|e| e.id == "start")
        {
            let result = event.result.clone().unwrap();
            assert!(
                result.member_errors.is_empty(),
                "{:?}",
                result.member_errors
            );
            break result.workspace;
        }
        assert!(Instant::now() < deadline, "workspace did not start");
        std::thread::sleep(Duration::from_millis(2));
    };
    let address = app
        .world()
        .get_resource::<axum_plugin::AxumHandle>()
        .unwrap()
        .local_address()
        .unwrap();
    let (headers, index) = get_static(address, "/");
    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: text/html")
    );
    let index = String::from_utf8(index).unwrap();
    assert!(index.contains("<div id=\"app\"></div>"));
    let script = index
        .split("src=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let (headers, script) = get_static(address, script);
    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: text/javascript")
    );
    assert!(!script.is_empty());
    let agent = *app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .agents
        .first()
        .unwrap();
    assert_eq!(
        app.world().get_component::<Agent>(agent).unwrap().state,
        AgentState::Idle
    );
    command(
        &mut app,
        agent,
        "INJECT ? TO entry.pending[-1]",
        vec![json!([{"type":"user","content":"Exercise the new stack."}])],
    );
    command(&mut app, agent, "EFFECT wake", vec![]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        let storage = app.world().get_component::<AgentStorage>(agent).unwrap();
        let StorageValue::HistoryEntries(records) =
            storage_core::execute_storage(storage, StorageOperation::ReadHistoryEntries).unwrap()
        else {
            panic!()
        };
        if records.iter().any(|r| {
            r.record.payload.get("content").and_then(|v| v.as_str())
                == Some("Finished through the new stack.")
        }) && app.world().get_component::<Agent>(agent).unwrap().state == AgentState::Idle
        {
            assert!(
                records
                    .iter()
                    .filter(|r| r.record.kind == "message")
                    .all(|r| r.sequence > 0 && r.record.created_at_ms.is_some())
            );
            let assistant = records
                .iter()
                .find(|r| {
                    r.record.payload.get("type").and_then(|v| v.as_str()) == Some("assistant")
                })
                .unwrap();
            assert!(assistant.record.payload["request_id"].as_str().is_some());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "driver did not finish inference and history"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let request = request_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["function"]["name"] == "task_read")
    );
    provider.join().unwrap();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        workspace,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|e| e.id == "stop")
        {
            event.result.clone().unwrap();
            break;
        }
        assert!(Instant::now() < deadline, "workspace did not stop");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!app.world().is_alive(agent));
    assert!(!app.world().is_alive(workspace));
    assert!(directory.join("agents/coder/state.sqlite").is_file());
    assert!(directory.join("agents/coder/setting.toml").is_file());
}
