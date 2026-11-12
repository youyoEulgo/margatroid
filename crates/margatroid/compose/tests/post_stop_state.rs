use app_runtime_plugin::AppRunExt;
use axum_plugin::AxumHandle;
use compose::{ApplicationOptions, build_app};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const CHILD_ROOT: &str = "MARGATROID_POST_STOP_STATE_TEST_ROOT";

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

async fn send(socket: &mut Socket, kind: &str, id: &str, message: Value) {
    socket
        .send(Message::Text(
            json!({"type":kind,"id":id,"message":message})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}

async fn next(socket: &mut Socket, matches: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Ok(Message::Text(text))) = socket.next().await {
                let value: Value = serde_json::from_str(&text).unwrap();
                if matches(&value) {
                    return value;
                }
            }
        }
    })
    .await
    .expect("event-driven daemon did not answer on the original WebSocket")
}

/// The child uses the production event-driven runner. A manual app.tick loop
/// would hide retries that depend on a wall-clock backoff but schedule no wake.
#[test]
fn original_websocket_can_query_inactive_workspaces_and_no_agents_after_last_stop() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        let mut app = build_app(ApplicationOptions {
            data_root: root.clone(),
            runner_path: std::env::current_exe().unwrap(),
        })
        .unwrap();
        app.tick();
        let deadline = Instant::now() + Duration::from_secs(5);
        let address = loop {
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
        std::fs::write(root.join("server-address"), address.to_string()).unwrap();
        app.run();
        unreachable!();
    }
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources"),
        root.path(),
    );
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let directory = root.path().join("workspaces/post-stop");
    std::fs::create_dir_all(&directory).unwrap();
    let definition = directory.join("workspace.yaml");
    std::fs::write(&definition, format!(
        "name: post-stop\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/coder:latest\n    home_root: {}\n",
        project.display(), root.path().display())).unwrap();
    std::fs::write(root.path().join("config.toml"),
        "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:unmatched']\nbackend_state=['type:webui']\n").unwrap();
    // The route is configuration only. No user messages or inference effects
    // are submitted, and this test does not need a provider or external network.
    let driver_model = agent_driver_core::read_driver(
        &root
            .path()
            .join("agent-drivers/local/coder/latest/driver.toml"),
        &Default::default(),
    )
    .unwrap()
    .manifest
    .inference
    .model;
    std::fs::write(root.path().join("models.toml"),
        format!("[[models]]\nid={driver_model:?}\nmodel='unused'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused-local-test'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n")).unwrap();
    let mut daemon = Daemon(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_websocket_can_query_inactive_workspaces_and_no_agents_after_last_stop",
                "--nocapture",
            ])
            .env(CHILD_ROOT, root.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let address = loop {
        if let Ok(address) = std::fs::read_to_string(root.path().join("server-address")) {
            break address;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon test child exited before startup"
        );
        assert!(Instant::now() < deadline, "daemon test child did not start");
        std::thread::sleep(Duration::from_millis(5));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        send(
            &mut socket,
            "connection.register",
            "register",
            json!({"client_type":"webui","name":"post-stop-regression"}),
        )
        .await;
        next(&mut socket, |v| v["type"] == "connection.register.result").await;
        for cycle in 0..3 {
            let start_id = format!("start-{cycle}");
            send(
                &mut socket,
                "workspace.start",
                &start_id,
                json!({"path":definition}),
            )
            .await;
            let started = next(&mut socket, |v| v["id"] == start_id).await;
            assert_eq!(started["type"], "workspace.start.result", "{started}");
            assert_eq!(started["success"], true, "{started}");
            assert!(started.get("workspace").is_none());
            let agents_id = format!("live-{cycle}");
            send(&mut socket, "agent.req", &agents_id, json!({})).await;
            let agents = next(&mut socket, |v| v["id"] == agents_id).await;
            assert_eq!(agents["type"], "agent.res");
            assert_eq!(agents["agents"].as_array().unwrap().len(), 1);
            let stop_id = format!("stop-{cycle}");
            send(
                &mut socket,
                "workspace.stop",
                &stop_id,
                json!({"path":definition}),
            )
            .await;
            let stopped = next(&mut socket, |v| v["id"] == stop_id).await;
            assert_eq!(stopped["type"], "workspace.stop.result", "{stopped}");
            assert_eq!(stopped["success"], true, "{stopped}");
            // No second client, periodic tick, or extra wake is allowed to
            // rescue this query during the snapshot failure retry backoff.
            for query in 0..2 {
                let id = format!("workspaces-{cycle}-{query}");
                send(&mut socket, "workspace.req", &id, json!({})).await;
                let state = next(&mut socket, |v| v["id"] == id).await;
                assert_eq!(state["type"], "workspace.res");
                assert_eq!(state["success"], true);
                let entries = state["workspaces"].as_array().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0]["path"], definition.to_string_lossy().as_ref());
                assert_eq!(entries[0]["status"], "inactive");
                assert!(entries[0]["address"].is_null());
                assert!(state.get("histories").is_none());
                let id = format!("agents-{cycle}-{query}");
                send(&mut socket, "agent.req", &id, json!({})).await;
                let state = next(&mut socket, |v| v["id"] == id).await;
                assert_eq!(state["type"], "agent.res");
                assert_eq!(state["success"], true);
                assert_eq!(state["agents"], json!([]));
            }
        }
        socket.close(None).await.unwrap();
    });
}
