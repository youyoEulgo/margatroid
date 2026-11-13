#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tool_core::ToolExecutionLimits;

async fn receive(output: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    let size = tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(size > 0, "supervisor closed before cleanup proof");
    serde_json::from_str(&line).unwrap()
}
async fn send(input: &mut tokio::process::ChildStdin, value: Value) {
    input
        .write_all((value.to_string() + "\n").as_bytes())
        .await
        .unwrap();
}

async fn external_allocation(detached: bool) {
    let root = tempfile::tempdir().unwrap();
    let payload = format!(
        r#"import os,sys,json
{}
data=bytearray(64*1024*1024)
for i in range(0,len(data),4096):data[i]=1
print(json.dumps({{'pid':os.getpid(),'allocated':len(data)}}),flush=True)
sys.stdin.read()
"#,
        if detached {
            "if os.fork():sys.exit(0)\nos.setsid()"
        } else {
            ""
        }
    );
    let program = format!(
        r#"
function configure() return {{timeout_ms=5000}} end
function execute()
    local child=margatroid.process:start('/usr/bin/python3',{{'-u','-c',{payload}}})
    local output=child:read()
    margatroid.task:return_result(output.data)
end
"#,
        payload = serde_json::to_string(&payload).unwrap()
    );
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .arg("--margatroid-tool-supervisor")
        .current_dir(root.path())
        .env(
            "MARGATROID_TOOL_CGROUP_ROOT",
            root.path().join("nonexistent-cgroup"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    send(&mut input,json!({"protocol_version":1,"program":program,"arguments":{},
        "context":{"agent_name":"fixture","turn":"turn","source_origin":"fixture.lua","task_id":"memory","call_id":"call","alias":"memory","resource_dir":root.path(),"project_root":root.path(),"temp_dir":root.path()},
        "limits":ToolExecutionLimits{lua_vm_memory_bytes:2*1024*1024,..Default::default()}})).await;
    let configured = receive(&mut output).await;
    assert_eq!(configured["frame"]["type"], "configured", "{configured}");
    send(
        &mut input,
        json!({"protocol_version":1,"frame":{"type":"configured"}}),
    )
    .await;
    let mut external_pid = None;
    loop {
        let wire = receive(&mut output).await;
        let frame = &wire["frame"];
        match frame["type"].as_str().unwrap() {
            "return" => {
                let proof: Value = serde_json::from_str(frame["value"].as_str().unwrap()).unwrap();
                assert_eq!(proof["allocated"], 64 * 1024 * 1024);
                external_pid = proof["pid"].as_u64();
                send(&mut input,json!({"protocol_version":1,"frame":{"type":"response","request_id":frame["request_id"],"result":{"Ok":null}}})).await;
            }
            "stopped" => {
                assert_eq!(frame["cleanup_proven"], true, "{wire}");
                assert_eq!(frame["result"], json!({"Ok":null}), "{wire}");
                break;
            }
            "lua_diagnostic" => panic!("unexpected diagnostic: {wire}"),
            _ => panic!("unexpected frame: {wire}"),
        }
    }
    assert!(tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    let pid = external_pid.expect("child must allocate and report its PID");
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "external process survived real cleanup"
    );
}

#[tokio::test]
async fn lua_limit_does_not_cap_external_process_memory_or_require_cgroups() {
    external_allocation(false).await;
}
#[tokio::test]
async fn detached_external_process_exceeds_lua_limit_but_is_still_reaped() {
    external_allocation(true).await;
}
