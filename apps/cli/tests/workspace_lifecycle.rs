#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

async fn receive(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
            other => panic!("unexpected message: {other:?}"),
        }
    }
}

#[tokio::test]
async fn member_failure_keeps_cli_alive_until_joined_stop() {
    exercise_member_failure(false).await;
}

#[tokio::test]
async fn shutdown_during_startup_waits_for_result_before_stopping() {
    exercise_member_failure(true).await;
}

async fn exercise_member_failure(shutdown_during_startup: bool) {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("workspace.yaml");
    std::fs::write(&config, "name: cli-test\nmanager: coder\nagents:\n  coder:\n    driver: local/coder:latest\n  reviewer:\n    driver: local/reviewer:latest\n").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .args(["workspace", "up"])
        .arg(&config)
        .args([
            "--backend",
            &format!("ws://{}", listener.local_addr().unwrap()),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let interaction = tokio::time::timeout(Duration::from_secs(10), async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let register = receive(&mut socket).await;
        socket.send(Message::Text(json!({"type":"connection.registered","id":register["id"],"client":{"resource_id":"client:cli/test:1","client_type":"cli","name":"test"}}).to_string().into())).await.unwrap();
        let start = receive(&mut socket).await;
        let definition = &start["message"]["definition"];
        let workspace = json!({"id":definition["id"],"name":definition["name"],"project_root":definition["project_root"]});
        if shutdown_during_startup {
            assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
            socket.send(Message::Ping(vec![4, 5].into())).await.unwrap();
            assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Pong(bytes) if bytes.as_ref()==[4,5]));
            assert!(child.try_wait().unwrap().is_none());
        }
        socket.send(Message::Text(json!({"type":"workspace.started","id":start["id"],"workspace":{
            "id":definition["id"],"name":definition["name"],"project_root":definition["project_root"],
            "manager":"agent:cli-test/coder:latest","agents":["agent:cli-test/coder:latest","agent:cli-test/reviewer:latest"],
            "member_failures":[{"agent":"agent:cli-test/reviewer:latest","driver":"driver:local/reviewer:latest","error":"cannot inspect driver path /missing/agent-drivers/local/reviewer/latest: No such file or directory"}]
        }}).to_string().into())).await.unwrap();
        socket.send(Message::Text(json!({"type":"workspace.start_failed","id":start["id"],"error":{"kind":"Stale","message":"late duplicate result"}}).to_string().into())).await.unwrap();
        if !shutdown_during_startup {
            socket.send(Message::Ping(vec![7, 8].into())).await.unwrap();
            assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Pong(bytes) if bytes.as_ref()==[7,8]));
            assert!(child.try_wait().unwrap().is_none());
            assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
        }
        let stop = receive(&mut socket).await;
        assert_eq!(stop["type"], "workspace.stop");
        assert!(child.try_wait().unwrap().is_none());
        socket.send(Message::Text(json!({"type":"workspace.stopped","id":stop["id"],"workspace":workspace}).to_string().into())).await.unwrap();
        while let Some(message) = socket.next().await {
            if matches!(message.unwrap(), Message::Close(_)) { break; }
        }
    }).await;
    if interaction.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        interaction.is_ok(),
        "CLI lifecycle timed out: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "workspace started",
        "reviewer",
        "driver:local/reviewer:latest",
        "/missing/agent-drivers/local/reviewer/latest",
        "workspace stopped",
    ] {
        assert!(
            diagnostics.contains(expected),
            "missing {expected}: {diagnostics}"
        );
    }
}
