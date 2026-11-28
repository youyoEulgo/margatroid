use std::process::Command;
#[test]
fn daemon_creates_initial_directories_but_requires_explicit_configuration() {
    let home = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_margatroid"))
        .env(
            if cfg!(windows) { "USERPROFILE" } else { "HOME" },
            home.path(),
        )
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("config"));
    for directory in [
        "workspaces",
        "agent-drivers",
        "prompts",
        "sandboxes",
        "tools",
    ] {
        assert!(home.path().join(".margatroid").join(directory).is_dir());
    }
    assert!(!home.path().join(".margatroid/models.toml").exists());
}
