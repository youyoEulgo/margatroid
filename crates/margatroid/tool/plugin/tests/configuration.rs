#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tool_core::ToolExecutionLimits;

async fn send(input: &mut tokio::process::ChildStdin, frame: Value) {
    let mut bytes = serde_json::to_vec(&frame).unwrap();
    bytes.push(b'\n');
    input.write_all(&bytes).await.unwrap();
}

async fn receive(output: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn runner(
    program: &str,
    arguments: Value,
) -> (
    tempfile::TempDir,
    tokio::process::Child,
    tokio::process::ChildStdin,
    BufReader<tokio::process::ChildStdout>,
) {
    let root = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .arg("--margatroid-tool-supervisor")
        .current_dir(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    send(&mut input, json!({
        "protocol_version": 1, "program": program, "arguments": arguments,
        "context": {"task_id":"configure-test", "call_id":"call", "alias":"tool", "project_root":root.path(), "temp_dir":root.path()},
        "limits": ToolExecutionLimits::default()
    })).await;
    (root, child, input, output)
}

#[tokio::test]
async fn invalid_or_missing_configuration_never_executes() {
    for configure in [
        "",
        "function configure() return nil end",
        "function configure() return string.rep('x', 100000000) end",
        "function configure() return {} end",
        "function configure() return {timeout_ms=1.5} end",
        "function configure() return {timeout_ms=-1} end",
        "function configure() return {timeout_ms=20,other=1} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=10} end",
        "function configure() return {timeout_ms=0,host_call_timeout_ms=10,cleanup_timeout_ms=10} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=1.5,cleanup_timeout_ms=10} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=10,cleanup_timeout_ms=10,other=1} end",
        "function configure() shell('touch unsafe') end",
        "function configure() while true do end end",
        "while true do end",
        "while true do pcall(function() while true do end end) end",
        "function configure() coroutine.resume(coroutine.create(function() while true do end end)) end",
        "io.open('unsafe', 'w')",
        "dofile('unsafe')",
    ] {
        let program = format!("{configure}\nfunction execute() shell('touch unsafe') end");
        let (root, mut child, _input, mut output) = runner(&program, json!({})).await;
        let frame = receive(&mut output).await;
        assert_eq!(frame["frame"]["type"], "stopped", "{configure}: {frame}");
        assert_eq!(
            frame["frame"]["result"]["Err"]["kind"], "InvalidDefinition",
            "{configure}: {frame}"
        );
        assert_eq!(frame["frame"]["cleanup_proven"], true);
        assert!(!root.path().join("unsafe").exists());
        assert!(child.wait().await.unwrap().success());
    }
}

#[tokio::test]
async fn configuration_uses_arguments_and_execution_waits_for_host_acknowledgement() {
    let program = r#"
        loads = (loads or 0) + 1
        assert(io == nil and os == nil and shell == nil and task_return == nil and print == nil and pcall == nil and coroutine == nil)
        function configure(arguments, context)
            assert(context.task_id == 'configure-test')
            return {timeout_ms=arguments.budget}
        end
        function execute()
            assert(loads == 1)
            shell('touch executed')
            task_return('done')
        end
    "#;
    let (root, mut child, mut input, mut output) = runner(program, json!({"budget":500})).await;
    let configured = receive(&mut output).await;
    assert_eq!(configured["frame"]["type"], "configured");
    assert_eq!(configured["frame"]["configuration"]["timeout_ms"], 500);
    let mut line = String::new();
    assert!(
        tokio::time::timeout(Duration::from_millis(550), output.read_line(&mut line))
            .await
            .is_err()
    );
    assert!(!root.path().join("executed").exists());
    send(
        &mut input,
        json!({"protocol_version":1,"frame":{"type":"configured"}}),
    )
    .await;
    let returned = receive(&mut output).await;
    assert_eq!(returned["frame"]["type"], "return");
    assert!(root.path().join("executed").exists());
    send(&mut input, json!({"protocol_version":1,"frame":{"type":"response","request_id":returned["frame"]["request_id"],"result":{"Ok":null}}})).await;
    let stopped = receive(&mut output).await;
    assert_eq!(stopped["frame"]["result"], json!({"Ok":null}));
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn explicit_execution_budget_stops_a_sleeping_shell_and_proves_cleanup() {
    let program =
        "function configure() return {timeout_ms=30} end function execute() shell('sleep 30') end";
    let (_root, mut child, mut input, mut output) = runner(program, json!({})).await;
    assert_eq!(receive(&mut output).await["frame"]["type"], "configured");
    send(
        &mut input,
        json!({"protocol_version":1,"frame":{"type":"configured"}}),
    )
    .await;
    let stopped = receive(&mut output).await;
    assert_eq!(stopped["frame"]["result"]["Err"]["kind"], "Timeout");
    assert_eq!(stopped["frame"]["cleanup_proven"], true);
    assert!(child.wait().await.unwrap().success());
}
