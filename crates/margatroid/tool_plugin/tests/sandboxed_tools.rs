//! End-to-end checks for a tool script running the way the daemon runs it.
//!
//! These load a tool script from the on-disk agent image and execute it exactly
//! as a tool call would, so the shell the runner injects is exercised for real
//! rather than through a stub.
//!
//! `tool_scripts.rs` covers the tool logic; this file adds the sandbox around it,
//! because a confining policy changes the runner's own lifecycle — it is the
//! outermost process in a process group the host reclaims when the call ends.

use std::path::{Path, PathBuf};
use std::process::Command;

fn runner_path() -> Option<PathBuf> {
    // The workspace target directory sits three levels above this crate:
    // crates/margatroid/tool_plugin -> crates/margatroid -> crates -> <root>.
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/tool_runner");
    candidate.is_file().then_some(candidate)
}

fn sandbox_backend() -> Option<PathBuf> {
    let path = std::env::var_os("MARGATROID_SANDBOX_BACKEND")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from("/home/eulgo/.local/bin/landstrip")))?;
    path.is_file().then_some(path)
}

fn image_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".margatroid/agent-images");
    path.is_dir().then_some(path)
}

fn policy(tier: &str) -> Option<PathBuf> {
    let image = image_root()?;
    let path = image
        .join("local/coder/latest/sandboxes/local")
        .join(tier)
        .join("latest/policy.json");
    path.is_file().then_some(path)
}

/// Everything the checks need, or `None` to skip where the machine lacks it.
fn ready(tier: &str) -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    Some((
        runner_path()?,
        sandbox_backend()?,
        policy(tier)?,
        image_root()?,
    ))
}

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "margatroid-sandboxed-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

/// A tool that reports whether it got a terminal, so the two policies can be
/// told apart by what the command actually saw rather than by assumption.
const PROBE: &str = r#"
function execute(arguments, context)
    local result = margatroid().shell("test -t 1 && echo tty || echo pipe")
    return margatroid().json.encode({
        ok = true,
        content = tostring(result.terminal) .. " " .. (result.stdout or ""):gsub("%s+", ""),
    })
end
"#;

fn probe_tool(project: &Path, image: &Path) -> PathBuf {
    let package = project.join("probe");
    std::fs::create_dir_all(&package).expect("probe package");
    std::fs::write(
        package.join("tool.toml"),
        "schema_version = 1\nname = \"probe\"\ndescription = \"reports the terminal\"\n",
    )
    .expect("probe metadata");
    std::fs::write(package.join("input.schema.json"), r#"{"type":"object"}"#)
        .expect("probe schema");
    std::fs::write(package.join("main.lua"), PROBE).expect("probe script");
    let _ = image;
    package
}

/// The writable policy grants the pty device, so a command gets a terminal.
#[test]
fn a_writable_sandbox_gives_the_command_a_terminal() {
    let Some((runner, backend, policy, image)) = ready("workspace-write") else {
        eprintln!("skipping: the sandbox path is unavailable");
        return;
    };
    let project = scratch("tty-write");
    let package = probe_tool(&project, &image);
    let script = project.join("main.sh");
    std::fs::write(&script, "exec bash -c \"$1\"\n").unwrap();

    let request = request_for("probe", &package, &project, &script);
    let output = run_with(&backend, &policy, &runner, &request).expect("sandboxed run");
    let value: serde_json::Value = serde_json::from_str(&output).expect("outcome envelope");
    let content = value["content"].as_str().unwrap();
    assert!(
        content.starts_with("true"),
        "the writable policy should grant a terminal: {content}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

/// The read-only policy withholds the pty device. The call must still succeed on
/// pipes — losing a terminal is not a reason to fail a tool.
#[test]
fn a_read_only_sandbox_falls_back_to_pipes_and_still_runs() {
    let Some((runner, backend, policy, image)) = ready("read-only") else {
        eprintln!("skipping: the sandbox path is unavailable");
        return;
    };
    let project = scratch("tty-read");
    let package = probe_tool(&project, &image);
    let script = project.join("main.sh");
    std::fs::write(&script, "exec bash -c \"$1\"\n").unwrap();

    let request = request_for("probe", &package, &project, &script);
    let output = run_with(&backend, &policy, &runner, &request).expect("sandboxed run");
    let value: serde_json::Value = serde_json::from_str(&output).expect("outcome envelope");
    let content = value["content"].as_str().unwrap();
    assert!(
        content.starts_with("false"),
        "the read-only policy withholds the terminal: {content}"
    );
    assert!(
        content.contains("pipe"),
        "the command should have run anyway: {content}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

/// The real bash tool, behind a confining policy, completes and reports its
/// outcome — the path that a terminal the runner failed to acquire used to break.
#[test]
fn the_bash_tool_completes_behind_a_confining_policy() {
    let Some((runner, backend, policy, image)) = ready("workspace-write") else {
        eprintln!("skipping: the sandbox path is unavailable");
        return;
    };
    let project = scratch("bash-confined");
    let package = image.join("local/coder/latest/tools/local/bash/latest");
    let script = project.join("main.sh");
    std::fs::write(&script, "exec bash -c \"$1\"\n").unwrap();

    let request = request_for("bash", &package, &project, &script);
    let output = run_with(&backend, &policy, &runner, &request).expect("sandboxed run");
    let value: serde_json::Value = serde_json::from_str(&output).expect("outcome envelope");
    assert_eq!(value["ok"], false, "a non-zero exit is a failure: {output}");
    assert!(
        value["content"].as_str().unwrap().contains("hello"),
        "the command's output should survive: {output}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

fn request_for(tool: &str, package: &Path, project: &Path, script: &Path) -> serde_json::Value {
    serde_json::json!({
        "metadata": std::fs::read_to_string(package.join("tool.toml")).unwrap(),
        "schema": std::fs::read_to_string(package.join("input.schema.json")).unwrap(),
        "script": std::fs::read_to_string(package.join("main.lua")).unwrap(),
        "temp_dir": project.to_string_lossy(),
        "arguments": if tool == "bash" {
            r#"{"cmd":"echo hello; exit 3"}"#
        } else {
            "{}"
        },
        "agent_id": "agent:local/coder:latest",
        "turn_id": "turn-1",
        "resource_id": format!("tool:local/{tool}:latest"),
        "project_root": project.to_string_lossy(),
        "shell_script": script.to_string_lossy(),
        "limits": {
            "max_definition_bytes": 1 << 20,
            "max_script_bytes": 1 << 20,
            "max_argument_bytes": 1 << 20,
            "max_output_bytes": 1 << 20,
            "max_memory_bytes": 1 << 28,
            "max_instructions": 1_000_000_000u64,
            "max_execution_time_ms": 30_000u64,
            "max_host_call_time_ms": 30_000u64,
        },
    })
}

fn run_with(
    backend: &Path,
    policy: &Path,
    runner: &Path,
    request: &serde_json::Value,
) -> Result<String, String> {
    let mut child = Command::new(backend)
        .arg("run")
        .arg("-p")
        .arg(policy)
        .arg("--")
        .arg(runner)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("sandboxed runner: {error}"))?;
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("runner stdin is unavailable")?;
        stdin
            .write_all(request.to_string().as_bytes())
            .map_err(|error| error.to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "runner exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}
