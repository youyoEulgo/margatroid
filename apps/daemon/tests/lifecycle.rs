use std::process::Command;
#[test]
fn daemon_reports_missing_startup_configuration_without_creating_workspaces() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".margatroid")).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_margatroid-daemon"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("config"));
    assert!(!home.path().join(".margatroid/workspaces").exists());
}
