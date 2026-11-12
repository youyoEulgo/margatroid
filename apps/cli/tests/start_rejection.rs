use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn workspace_own_start_failure_exits_cli_without_stop_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .args(["workspace", "up"])
        .arg("cli-test")
        .args([
            "--backend",
            &format!("ws://{}", listener.local_addr().unwrap()),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let register = socket.next().await.unwrap().unwrap();
        let register: Value = serde_json::from_str(register.to_text().unwrap()).unwrap();
        socket.send(Message::Text(json!({"type":"connection.register.result","id":register["id"],"success":true,"client":{"address":"client:cli/test:1","client_type":"cli"}}).to_string().into())).await.unwrap();
        let query = socket.next().await.unwrap().unwrap();
        let query: Value = serde_json::from_str(query.to_text().unwrap()).unwrap();
        assert_eq!(query["type"], "workspace.req");
        socket.send(Message::Text(json!({"type":"workspace.res","id":query["id"],"success":true,"workspaces":[{"path":"/registered/cli-test/workspace.yaml","definition":{"name":"cli-test","project_root":"/project","agents":[]},"status":"inactive","address":null,"error":null}]}).to_string().into())).await.unwrap();
        let start = socket.next().await.unwrap().unwrap();
        let start: Value = serde_json::from_str(start.to_text().unwrap()).unwrap();
        assert_eq!(start["message"],json!({"path":"/registered/cli-test/workspace.yaml"}));
        socket.send(Message::Text(json!({"type":"workspace.start.result","id":start["id"],"success":false,"error":{"code":"ExecutionStartFailed","message":"cannot create workspace execution thread"}}).to_string().into())).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message {
                panic!("CLI must not stop a workspace that was never created: {text}");
            }
        }
    }).await;
    if result.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        result.is_ok(),
        "CLI did not exit after workspace startup failure"
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("cannot create workspace execution thread"));
}
