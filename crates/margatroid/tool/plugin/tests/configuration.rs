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
        "context": {"task_id":"configure-test", "call_id":"call", "alias":"tool", "owner_task_id":null,"binding":null,"resource_dir":root.path(),"project_root":root.path(), "temp_dir":root.path()},
        "limits": ToolExecutionLimits::default()
    })).await;
    (root, child, input, output)
}

#[tokio::test]
async fn invalid_or_missing_configuration_never_executes() {
    for configure in [
        "",
        "function configure() return nil end",
        "function configure() return string.rep('x', 1024) end",
        "function configure() return {} end",
        "function configure() return {timeout_ms=1.5} end",
        "function configure() return {timeout_ms=-1} end",
        "function configure() return {timeout_ms=20,other=1} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=10} end",
        "function configure() return {timeout_ms=0,host_call_timeout_ms=10,cleanup_timeout_ms=10} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=1.5,cleanup_timeout_ms=10} end",
        "function configure() return {timeout_ms=20,host_call_timeout_ms=10,cleanup_timeout_ms=10,other=1} end",
        "function configure() margatroid:process_open({program='/bin/touch',arguments={'unsafe'}}) end",
        "function configure() margatroid:task_return('unsafe') end",
        "function configure() margatroid:json_encode({unsafe=true}) end",
        "function configure() margatroid:base64_encode('unsafe') end",
        "function configure() margatroid:await_any({}) end",
        "function configure() margatroid.version=2 end",
        "function configure() margatroid.context={} end",
        "function configure() margatroid.arguments={} end",
        "while true do pcall(function() while true do end end) end",
        "function configure() coroutine.resume(coroutine.create(function() while true do end end)) end",
        "io.open('unsafe', 'w')",
        "dofile('unsafe')",
    ] {
        let program = format!("{configure}\nfunction execute() margatroid:process_open({{program='/bin/touch',arguments={{'unsafe'}}}}):wait() end");
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
async fn infinite_loading_or_configuration_is_cancelled_by_host_disconnect() {
    for program in [
        "while true do end",
        "function configure() while true do end end function execute() error('must not execute') end",
    ] {
        let (root, mut child, input, mut output) = runner(program, json!({})).await;
        let mut line = String::new();
        assert!(tokio::time::timeout(Duration::from_millis(30), output.read_line(&mut line)).await.is_err());
        drop(input);
        let frame = receive(&mut output).await;
        assert_eq!(frame["frame"]["type"], "stopped", "{program}: {frame}");
        assert_eq!(frame["frame"]["result"]["Err"]["kind"], "Cancelled", "{program}: {frame}");
        assert_eq!(frame["frame"]["cleanup_proven"], true);
        assert!(!root.path().join("unsafe").exists());
        assert!(child.wait().await.unwrap().success());
    }
}

#[tokio::test]
async fn configuration_uses_arguments_and_execution_waits_for_host_acknowledgement() {
    let program = r#"
        loads = (loads or 0) + 1
        for _, name in ipairs({'io','os','process_open','task_return','print','pcall','coroutine'}) do
            assert(_G[name] == nil, 'unexpected configure global: '..name)
        end
        assert(type(margatroid) == 'userdata' and margatroid.version == 1)
        assert(context == nil and args == nil and json == nil and base64 == nil and await_any == nil)
        assert(margatroid.context.task_id == 'configure-test')
        assert(margatroid.arguments.budget == 500)
        function configure(arguments, context)
            assert(context.task_id == margatroid.context.task_id)
            assert(arguments.budget == margatroid.arguments.budget)
            return {timeout_ms=margatroid.arguments.budget}
        end
        function execute()
            assert(loads == 1)
            margatroid:process_open({program='/bin/touch',arguments={'executed'}}):wait()
            margatroid:task_return('done')
        end
    "#;
    let (root, mut child, mut input, mut output) = runner(program, json!({"budget":500})).await;
    let configured = receive(&mut output).await;
    assert_eq!(configured["frame"]["type"], "configured", "{configured}");
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
async fn explicit_execution_budget_stops_a_sleeping_process_and_proves_cleanup() {
    let program =
        "function configure() return {timeout_ms=30} end function execute() margatroid:process_open({program='/bin/sh',arguments={'-c','sleep 30'}}):wait() end";
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
