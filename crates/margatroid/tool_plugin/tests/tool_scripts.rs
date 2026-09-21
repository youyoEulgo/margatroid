//! End-to-end checks for a tool script running through the real runner.
//!
//! These load a tool script from the on-disk agent image and execute it exactly
//! as a tool call would, so the shell the runner injects is exercised for real
//! rather than through a stub.

use std::path::{Path, PathBuf};
use std::time::Duration;

use margatroid_types::ResourceId;
use tool_plugin::{run_lua_tool, LuaExecutionLimits, LuaToolRunRequest};

fn runner_path() -> Option<PathBuf> {
    // The integration test binary sits next to the runner in target/debug.
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/tool_runner");
    candidate.is_file().then_some(candidate)
}

fn image_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".margatroid/agent-images");
    path.is_dir().then_some(path)
}

/// The image is a local artifact rather than a repository file, so the checks
/// that need it announce a skip instead of failing where it is absent.
fn ready(name: &str) -> Option<(PathBuf, PathBuf)> {
    let runner = runner_path()?;
    let image = image_root()?;
    let package = image
        .join("local/coder/latest/tools/local")
        .join(name)
        .join("latest");
    if !package.join("main.lua").is_file() {
        return None;
    }
    Some((runner, package))
}

fn limits() -> LuaExecutionLimits {
    LuaExecutionLimits {
        max_definition_bytes: 1 << 20,
        max_script_bytes: 1 << 20,
        max_argument_bytes: 1 << 20,
        max_output_bytes: 1 << 20,
        max_memory_bytes: 1 << 28,
        max_instructions: 1_000_000_000,
        max_execution_time: Duration::from_secs(30),
        max_host_call_time: Duration::from_secs(30),
    }
}

/// Run one tool script from the coder image against a scratch project.
fn run_tool(
    tool: &str,
    arguments: &str,
    project_root: &Path,
    shell_script: Option<PathBuf>,
) -> Result<String, String> {
    let image = image_root().ok_or("the agent image root is not available")?;
    let package = image
        .join("local/coder/latest/tools/local")
        .join(tool)
        .join("latest");
    let metadata = std::fs::read_to_string(package.join("tool.toml"))
        .map_err(|error| format!("tool.toml: {error}"))?;
    let schema = std::fs::read_to_string(package.join("input.schema.json"))
        .map_err(|error| format!("input.schema.json: {error}"))?;
    let script = std::fs::read_to_string(package.join("main.lua"))
        .map_err(|error| format!("main.lua: {error}"))?;

    let request = LuaToolRunRequest {
        metadata,
        schema,
        script,
        temp_dir: project_root.to_path_buf(),
        arguments: arguments.to_owned(),
        agent_id: ResourceId::parse("agent:local/coder:latest").unwrap(),
        turn_id: "turn-1".to_owned(),
        resource_id: ResourceId::parse(&format!("tool:local/{tool}:latest")).unwrap(),
        project_root: project_root.to_path_buf(),
        limits: limits(),
        sandbox_policies: Vec::new(),
        shell_script: shell_script.map(std::sync::Arc::new),
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime: {error}"))?;
    runtime
        .block_on(run_lua_tool(request))
        .map_err(|error| error.to_string())
}

fn scratch(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("margatroid-tool-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

fn stage_shell(project_root: &Path) -> PathBuf {
    let script = project_root.join("main.sh");
    std::fs::write(&script, "exec bash -lc \"$1\"\n").expect("shell script");
    script
}

#[test]
fn the_glob_tool_lists_files_through_the_injected_shell() {
    let Some((runner, _)) = ready("glob") else {
        eprintln!("skipping: the coder image or tool_runner is unavailable");
        return;
    };
    let project = scratch("glob");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(project.join("README.md"), "# hi\n").unwrap();
    let shell = stage_shell(&project);

    std::env::set_var("MARGATROID_TOOL_RUNNER", &runner);
    let result = run_tool("glob", r#"{"pattern":"**/*.rs"}"#, &project, Some(shell));
    std::env::remove_var("MARGATROID_TOOL_RUNNER");

    let output = result.expect("glob should run");
    assert!(output.contains("src/main.rs"), "missing main.rs: {output}");
    assert!(output.contains("src/lib.rs"), "missing lib.rs: {output}");
    assert!(!output.contains("README.md"), "md leaked in: {output}");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn the_grep_tool_reports_matches_and_no_match_is_not_an_error() {
    let Some((runner, _)) = ready("grep") else {
        eprintln!("skipping: the coder image or tool_runner is unavailable");
        return;
    };
    let project = scratch("grep");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/a.rs"), "let needle = 1;\n").unwrap();
    let shell = stage_shell(&project);

    std::env::set_var("MARGATROID_TOOL_RUNNER", &runner);
    let matched = run_tool(
        "grep",
        r#"{"pattern":"needle"}"#,
        &project,
        Some(shell.clone()),
    );
    // ripgrep exits 1 for "no match", which the tool must not treat as failure.
    let unmatched = run_tool(
        "grep",
        r#"{"pattern":"definitely-not-present"}"#,
        &project,
        Some(shell),
    );
    std::env::remove_var("MARGATROID_TOOL_RUNNER");

    let matched = matched.expect("grep should run");
    assert!(matched.contains("needle"), "match missing: {matched}");
    let unmatched = unmatched.expect("a no-match grep is not an error");
    assert!(
        unmatched.contains("No matches"),
        "expected the no-match wording: {unmatched}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn the_list_directory_tool_returns_entries_through_the_injected_shell() {
    let Some((runner, _)) = ready("list-directory") else {
        eprintln!("skipping: the coder image or tool_runner is unavailable");
        return;
    };
    let project = scratch("list");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    let shell = stage_shell(&project);

    std::env::set_var("MARGATROID_TOOL_RUNNER", &runner);
    let result = run_tool("list-directory", r#"{"path":"src"}"#, &project, Some(shell));
    std::env::remove_var("MARGATROID_TOOL_RUNNER");

    let output = result.expect("list-directory should run");
    assert!(output.contains("main.rs"), "entry missing: {output}");
    assert!(output.contains("\"file\""), "kind missing: {output}");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_tool_without_a_shell_reports_that_clearly() {
    let Some((runner, _)) = ready("glob") else {
        eprintln!("skipping: the coder image or tool_runner is unavailable");
        return;
    };
    let project = scratch("noshell");
    std::fs::write(project.join("a.txt"), "x\n").unwrap();

    std::env::set_var("MARGATROID_TOOL_RUNNER", &runner);
    let result = run_tool("glob", r#"{"pattern":"*.txt"}"#, &project, None);
    std::env::remove_var("MARGATROID_TOOL_RUNNER");

    let error = result.expect_err("a tool without a shell cannot search");
    assert!(
        error.contains("imports no shell"),
        "expected the no-shell wording: {error}"
    );
    let _ = std::fs::remove_dir_all(&project);
}
