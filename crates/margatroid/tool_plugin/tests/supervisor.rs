#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tool_plugin::{RunnerRequest, RunnerResponse, ToolContext, ToolErrorKind, ToolExecutionLimits};

fn request(root: &Path, script: &str) -> RunnerRequest {
    RunnerRequest {
        protocol_version: 1,
        script: script.into(),
        arguments: serde_json::json!({}),
        context: ToolContext {
            agent_id: "supervisor-test".into(),
            resource_id: "tool:test/supervisor:latest".into(),
            call_id: "call".into(),
            turn: None,
            project_root: root.into(),
            temp_dir: root.into(),
        },
        limits: ToolExecutionLimits {
            execution_timeout: Duration::from_secs(5),
            host_call_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    }
}

async fn spawn(request: &RunnerRequest, args: &[&std::ffi::OsStr]) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .args(args)
        .current_dir(&request.context.project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&serde_json::to_vec(request).unwrap())
        .await
        .unwrap();
    stdin.shutdown().await.unwrap();
    child
}

async fn response(child: tokio::process::Child) -> RunnerResponse {
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_dead(root: &Path) {
    let pid: i32 = std::fs::read_to_string(root.join("escaped.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "escaped descendant {pid} remains alive"
    );
}

const ESCAPE: &str = r#"
function execute(args, context)
    shell("setsid /bin/sh -c 'echo $$ > escaped.pid; exec sleep 60' </dev/null >/dev/null 2>&1 &")
    shell("while [ ! -s escaped.pid ]; do sleep 0.01; done")
    return "done"
end
"#;

#[tokio::test]
async fn default_supervisor_cleans_setsid_descendant() {
    let root = tempfile::tempdir().unwrap();
    let response = response(spawn(&request(root.path(), ESCAPE), &[]).await).await;
    assert!(response.cleanup_proven);
    assert_eq!(response.result.unwrap(), "done");
    assert_dead(root.path());
}

#[tokio::test]
async fn explicit_worker_never_claims_cleanup_proof() {
    let root = tempfile::tempdir().unwrap();
    let response = response(
        spawn(
            &request(root.path(), "function execute() return 'worker' end"),
            &[std::ffi::OsStr::new("--margatroid-tool-worker")],
        )
        .await,
    )
    .await;
    assert!(!response.cleanup_proven);
    assert_eq!(response.result.unwrap(), "worker");
}

#[tokio::test]
async fn supervisor_signals_cancel_and_clean_escaped_descendants() {
    for signal in [
        nix::sys::signal::Signal::SIGTERM,
        nix::sys::signal::Signal::SIGINT,
        nix::sys::signal::Signal::SIGHUP,
    ] {
        let root = tempfile::tempdir().unwrap();
        let script = ESCAPE.replace("return \"done\"", "shell('sleep 60'); return 'done'");
        let child = spawn(&request(root.path(), &script), &[]).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !root.path().join("escaped.pid").is_file() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(child.id().unwrap() as i32),
            signal,
        )
        .unwrap();
        let response = response(child).await;
        assert!(response.cleanup_proven);
        assert_eq!(response.result.unwrap_err().kind, ToolErrorKind::Cancelled);
        assert_dead(root.path());
    }
}

fn landstrip() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("landstrip"))
            .find(|path| path.is_file())
    })
}

#[tokio::test]
async fn real_landstrip_denies_proc_without_breaking_supervisor_proof() {
    let Some(backend) = landstrip() else {
        eprintln!("landstrip unavailable; skipping real-policy regression");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let policy = root.path().join("policy.json");
    std::fs::write(
        &policy,
        serde_json::to_vec(&serde_json::json!({
            "filesystem": {
                "allowRead": ["/"],
                "denyRead": ["/proc"],
                "allowWrite": [root.path(), "/dev/null"]
            },
            "network": {"allowNetwork": false, "allowLocalBinding": false}
        }))
        .unwrap(),
    )
    .unwrap();
    let probe = tokio::process::Command::new(&backend)
        .args(["run", "-p"])
        .arg(&policy)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "ls /proc >/dev/null 2>&1; test $? -ne 0",
        ])
        .status()
        .await
        .unwrap();
    assert!(probe.success(), "real sandbox must actually deny /proc");
    let args = [
        std::ffi::OsStr::new("--margatroid-tool-supervisor"),
        std::ffi::OsStr::new("--sandbox"),
        backend.as_os_str(),
        policy.as_os_str(),
    ];
    let response = response(spawn(&request(root.path(), ESCAPE), &args).await).await;
    assert!(response.cleanup_proven);
    assert_eq!(response.result.unwrap(), "done");
    assert_dead(root.path());
}
