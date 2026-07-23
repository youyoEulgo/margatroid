use std::fs;
use std::process::Command;

#[test]
fn workspace_config_compiles_without_a_daemon() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
workspace:
  name: local-test
  manager: coder
agents:
  coder:
    image: acme/coder:v1
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .current_dir(project.path())
        .args(["workspace", "config", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["spec"]["name"], "local-test");
    assert!(json.get("resources").is_none());
}

#[test]
fn workspace_config_accepts_a_bare_relative_file_and_directory_name_fallback() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("fallback-project");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("custom.yaml"),
        r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .current_dir(&project)
        .args([
            "workspace",
            "config",
            "-f",
            "custom.yaml",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["spec"]["name"], "fallback-project");
}

#[test]
fn workspace_config_errors_do_not_expose_absolute_paths() {
    let project = tempfile::tempdir().unwrap();
    let compose = project.path().join("broken.yaml");
    fs::write(&compose, "not: [valid").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .args(["workspace", "config", "-f", compose.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains(project.path().to_str().unwrap()),
        "{stderr}"
    );
    assert!(stderr.contains("broken.yaml:"), "{stderr}");
}
