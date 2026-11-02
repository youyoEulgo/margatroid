use app_runtime_plugin::AppRunExt;
use compose::{ApplicationOptions, build_app};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use server_plugin::ServerHandle;
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
fn original_websocket_can_query_empty_state_after_stopping_the_last_workspace() {
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
            if let Some(address) = app
                .world()
                .get_resource::<ServerHandle>()
                .and_then(ServerHandle::local_address)
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
    std::fs::write(root.path().join("models.toml"),
        "[[models]]\nid='openrouter/deepseek-v4.1-flash'\nmodel='unused'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused-local-test'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n").unwrap();
    let mut daemon = Daemon(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_websocket_can_query_empty_state_after_stopping_the_last_workspace",
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
        next(&mut socket, |v| v["type"] == "connection.registered").await;
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
            assert_eq!(started["type"], "workspace.started", "{started}");
            assert!(
                started["workspace"]["member_failures"]
                    .as_array()
                    .is_none_or(|v| v.is_empty())
            );
            let workspace = started["workspace"]["id"].as_str().unwrap();
            let stop_id = format!("stop-{cycle}");
            send(
                &mut socket,
                "workspace.stop",
                &stop_id,
                json!({"workspace":workspace}),
            )
            .await;
            let stopped = next(&mut socket, |v| v["id"] == stop_id).await;
            assert_eq!(stopped["type"], "workspace.stopped", "{stopped}");
            // No second client, periodic tick, or extra wake is allowed to
            // rescue this query during the snapshot failure retry backoff.
            for query in 0..2 {
                let id = format!("state-{cycle}-{query}");
                send(&mut socket, "state.query", &id, json!({})).await;
                let state = next(&mut socket, |v| {
                    v["type"] == "state.sync"
                        && v["state"]["workspaces"] == json!([])
                        && v["state"]["agents"] == json!([])
                })
                .await;
                assert_eq!(state["state"]["histories"], json!([]));
            }
        }
        socket.close(None).await.unwrap();
    });
}
