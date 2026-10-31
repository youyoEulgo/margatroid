#![cfg(target_os = "linux")]

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tool_core::ToolExecutionLimits;

#[tokio::test]
async fn unavailable_selected_backend_never_falls_back_to_unrestricted_worker() {
    let root = tempfile::tempdir().unwrap();
    let policy = root.path().join("policy.json");
    std::fs::write(&policy, "{}").unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .arg("--margatroid-tool-supervisor")
        .arg("--sandbox")
        .arg(root.path().join("missing-backend"))
        .arg(&policy)
        .current_dir(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let request = json!({
        "protocol_version":1,
        "program":"function execute() shell('touch unsafe'); task_return('unsafe') end",
        "arguments":{},
        "context":{"task_id":"sandbox-test","call_id":"call","alias":"tool","project_root":root.path(),"temp_dir":root.path()},
        "limits":ToolExecutionLimits::default()
    });
    let mut stdin = child.stdin.take().unwrap();
    let mut raw = serde_json::to_vec(&request).unwrap();
    raw.push(b'\n');
    stdin.write_all(&raw).await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frame: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(frame["frame"]["type"], "stopped");
    assert_eq!(frame["frame"]["cleanup_proven"], true);
    assert_eq!(frame["frame"]["result"]["Err"]["kind"], "RunnerStartFailed");
    assert!(!root.path().join("unsafe").exists());
}
