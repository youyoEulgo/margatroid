use std::io::Write;
use std::process::{Command, Stdio};
use tool_plugin::{RunnerRequest, RunnerResponse, ToolContext, ToolExecutionLimits};

fn run(
    root: &std::path::Path,
    script: &str,
    arguments: serde_json::Value,
    limits: ToolExecutionLimits,
) -> RunnerResponse {
    let request = RunnerRequest {
        protocol_version: 1,
        script: script.into(),
        arguments,
        context: ToolContext {
            agent_id: "agent:test/abi:latest".into(),
            resource_id: "tool:test/abi:latest".into(),
            call_id: "abi".into(),
            turn: None,
            project_root: root.into(),
            temp_dir: root.into(),
        },
        limits,
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("iVBORw0KGgo"),
        "image data leaked to diagnostics"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn read_image_real_runner_handles_paths_formats_and_limits() {
    use base64::Engine as _;
    let root = tempfile::tempdir().unwrap();
    let script = include_str!("../../../../resources/tools/local/read-image/latest/main.lua");
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1sAAAAASUVORK5CYII=").unwrap();
    let file = root.path().join("actual image.weird");
    std::fs::write(&file, &png).unwrap();
    for path in [
        "actual image.weird".to_owned(),
        file.to_string_lossy().into_owned(),
    ] {
        let response = run(
            root.path(),
            script,
            serde_json::json!({"path":path}),
            ToolExecutionLimits::default(),
        );
        assert!(response.cleanup_proven);
        let envelope: serde_json::Value = serde_json::from_str(&response.result.unwrap()).unwrap();
        assert_eq!(envelope["ok"], true);
        assert_eq!(envelope["images"][0]["media_type"], "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(envelope["images"][0]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
        assert!(!envelope["content"].as_str().unwrap().contains("iVBOR"));
    }
    std::fs::write(root.path().join("fake.png"), "not an image").unwrap();
    let mut maximum = vec![0u8; 5 * 1024 * 1024];
    maximum[..png.len()].copy_from_slice(&png);
    std::fs::write(root.path().join("maximum.png"), &maximum).unwrap();
    let response = run(
        root.path(),
        script,
        serde_json::json!({"path":"maximum.png"}),
        ToolExecutionLimits::default(),
    );
    assert!(response.cleanup_proven);
    let envelope: serde_json::Value = serde_json::from_str(&response.result.unwrap()).unwrap();
    assert_eq!(envelope["ok"], true);
    assert!(envelope["images"][0]["data"].as_str().unwrap().len() > 6 * 1024 * 1024);
    maximum.push(0);
    std::fs::write(root.path().join("oversize.png"), maximum).unwrap();
    for path in ["missing.png", "fake.png", "oversize.png"] {
        let response = run(
            root.path(),
            script,
            serde_json::json!({"path":path}),
            ToolExecutionLimits::default(),
        );
        assert!(response.cleanup_proven);
        let envelope: serde_json::Value = serde_json::from_str(&response.result.unwrap()).unwrap();
        assert_eq!(envelope["ok"], false);
        assert!(envelope.get("images").is_none());
    }
}

#[test]
fn deployed_tools_execute_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let tools = [
        (
            "write-file",
            serde_json::json!({"path":"nested/sample.txt", "content":"alpha\nbeta\n", "create_parent_directories":true}),
        ),
        ("read-file", serde_json::json!({"path":"nested/sample.txt"})),
        (
            "edit",
            serde_json::json!({"path":"nested/sample.txt", "old_string":"beta", "new_string":"gamma"}),
        ),
        ("list-directory", serde_json::json!({"path":"nested"})),
        ("glob", serde_json::json!({"pattern":"*.txt"})),
        ("grep", serde_json::json!({"pattern":"gamma"})),
        ("bash", serde_json::json!({"cmd":"printf deployed"})),
    ];
    for (name, arguments) in tools {
        let script = match name {
            "write-file" => include_str!("fixtures/deployed/write-file.lua"),
            "read-file" => include_str!("fixtures/deployed/read-file.lua"),
            "edit" => include_str!("fixtures/deployed/edit.lua"),
            "list-directory" => include_str!("fixtures/deployed/list-directory.lua"),
            "glob" => include_str!("fixtures/deployed/glob.lua"),
            "grep" => include_str!("fixtures/deployed/grep.lua"),
            "bash" => include_str!("fixtures/deployed/bash.lua"),
            _ => unreachable!(),
        };
        let response = run(
            root.path(),
            script,
            arguments,
            ToolExecutionLimits::default(),
        );
        assert!(response.cleanup_proven, "{name}");
        let result = response
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let envelope: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(envelope["ok"], true, "{name}: {result}");
        assert!(!envelope["content"].as_str().unwrap().is_empty(), "{name}");
    }
    assert_eq!(
        std::fs::read_to_string(root.path().join("nested/sample.txt")).unwrap(),
        "alpha\ngamma\n"
    );
}

#[test]
fn shell_caps_exit_status_and_standard_os_tuple() {
    let root = tempfile::tempdir().unwrap();
    let script = r#"
function execute()
    assert(package.loadlib == nil)
    assert(margatroid().version == 1)
    assert(margatroid().context.project_root == context.project_root)
    local small = margatroid().shell("printf abcdef; printf diagnostic >&2; exit 7", {max_output_bytes=3})
    assert(small.stdout == "abc" and small.stderr == "dia")
    assert(small.exit_code == 7 and small.code == 7 and small.truncated)
    local exact = shell("printf abc", {max_output_bytes=3})
    assert(exact.stdout == "abc" and not exact.truncated)
    local capped = shell("printf '%0300d' 1", {max_output_bytes=999999})
    assert(#capped.stdout == 128 and capped.truncated)
    local terminal = shell("printf abcdef", {pty=true, max_output_bytes=3})
    assert(terminal.stdout == "abc" and terminal.truncated and terminal.terminal)
    local ok, kind, code = os.execute("exit 0")
    assert(ok == true and kind == "exit" and code == 0)
    ok, kind, code = os.execute("exit 9")
    assert(ok == nil and kind == "exit" and code == 9)
    ok, kind, code = os.execute("kill -TERM $$")
    assert(ok == nil and kind == "signal" and code == 15)
    assert(margatroid().arguments ~= nil)
    io.write("diagnostic only")
    io.stdout:write("also diagnostic")
    return json.encode({ok=true, content="compatible"})
end
"#;
    let mut limits = ToolExecutionLimits::default();
    limits.output_bytes = 128;
    let response = run(root.path(), script, serde_json::json!({}), limits);
    assert!(response.cleanup_proven);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.result.unwrap()).unwrap()["content"],
        "compatible"
    );
}

#[test]
fn async_os_execute_obeys_host_timeout() {
    let root = tempfile::tempdir().unwrap();
    let mut limits = ToolExecutionLimits::default();
    limits.host_call_timeout = std::time::Duration::from_millis(100);
    let started = std::time::Instant::now();
    let response = run(
        root.path(),
        "function execute() os.execute('sleep 5'); return 'unexpected' end",
        serde_json::json!({}),
        limits,
    );
    assert!(response.cleanup_proven);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    let error = response.result.unwrap_err();
    assert_eq!(error.kind, tool_plugin::ToolErrorKind::Timeout);
    assert!(error.message.contains("shell deadline exceeded"), "{error}");
}
