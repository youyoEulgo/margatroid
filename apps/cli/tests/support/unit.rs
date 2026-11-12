use super::*;
use serde_json::json;

fn definition(name: &str, root: &Path) -> WorkspaceInfoDto {
    serde_json::from_value(json!({"path":format!("/configured/{name}/workspace.yaml"),"definition":{"name":name,"project_root":root,"agents":[]},"status":"inactive","address":null,"error":null})).unwrap()
}
#[test]
fn definitions_select_exact_name_or_canonical_current_project_root() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let entries = vec![
        definition("one", directory.path()),
        definition("two", directory.path()),
    ];
    assert_eq!(
        definition_candidates(&entries, None, &directory.path().canonicalize().unwrap()).len(),
        2
    );
    assert!(definition_candidates(&entries, None, &nested.canonicalize().unwrap()).is_empty());
    assert_eq!(
        definition_candidates(&entries, Some("two"), &nested)[0].path,
        "/configured/two/workspace.yaml"
    );
    assert!(
        definition_candidates(&entries, Some("/arbitrary/file.yaml"), directory.path()).is_empty()
    );
}
#[test]
fn chooser_cancel_and_invalid_numbers_never_guess() {
    let root = tempfile::tempdir().unwrap();
    let entries = [
        definition("one", root.path()),
        definition("two", root.path()),
    ];
    let candidates = entries.iter().collect::<Vec<_>>();
    assert_eq!(
        parse_choice("2", &candidates)
            .unwrap()
            .unwrap()
            .definition
            .name,
        "two"
    );
    for cancel in ["", "q", "Q"] {
        assert!(parse_choice(cancel, &candidates).unwrap().is_none());
    }
    for invalid in ["0", "3", "one", "-1"] {
        assert!(parse_choice(invalid, &candidates).is_err());
    }
}
#[test]
fn startup_receipt_confirms_operation_without_inventing_runtime_state() {
    let mut session = CliSession {
        workspace_path: Some("/configured/test/workspace.yaml".into()),
        ..CliSession::default()
    };
    let message = json!({"type":"workspace.start.result","id":"start","success":true}).to_string();
    handle_workspace_start_reply(&message, "other", &mut session).unwrap();
    assert!(!session.workspace_started);
    handle_workspace_start_reply(&message, "start", &mut session).unwrap();
    assert!(session.workspace_started);
    let late=json!({"type":"workspace.start.result","id":"start","success":false,"error":{"code":"Late","message":"duplicate"}}).to_string();
    handle_workspace_start_reply(&late, "start", &mut session).unwrap();
}
#[test]
fn matching_failure_before_start_is_terminal_and_unrelated_ids_are_ignored() {
    let message=json!({"type":"workspace.start.result","id":"start","success":false,"error":{"code":"InvalidRequest","message":"bad path"}}).to_string();
    let mut session = CliSession::default();
    handle_workspace_start_reply(&message, "other", &mut session).unwrap();
    assert!(
        handle_workspace_start_reply(&message, "start", &mut session)
            .unwrap_err()
            .to_string()
            .contains("bad path")
    );
    assert!(!session.workspace_started);
}
#[test]
fn registration_and_stop_receipts_require_the_expected_id() {
    let registration=json!({"type":"connection.register.result","id":"registration","success":false,"error":{"code":"Denied","message":"denied"}}).to_string();
    assert!(registration_reply(&registration, "other").is_none());
    assert!(matches!(
        registration_reply(&registration, "registration"),
        Some(Err(CliError::Remote(_)))
    ));
    let stop = json!({"type":"workspace.stop.result","id":"stop","success":true}).to_string();
    assert!(stop_reply(&stop, "other").is_none());
    assert!(matches!(stop_reply(&stop, "stop"), Some(Ok(()))));
    let failure=json!({"type":"workspace.stop.result","id":"stop","success":false,"error":{"code":"CleanupFailed","message":"not stopped"}}).to_string();
    assert!(matches!(
        stop_reply(&failure, "stop"),
        Some(Err(CliError::Remote(_)))
    ));
}
#[test]
fn command_surface_has_only_workspace_up_and_backend_option() {
    assert_eq!(
        Cli::try_parse_from(["margatroid", "workspace", "up"])
            .unwrap()
            .command,
        Command::Workspace {
            command: WorkspaceCommand::Up(WorkspaceUpArgs {
                name: None,
                backend_url: DEFAULT_BACKEND_URL.into()
            })
        }
    );
    assert!(Cli::try_parse_from([
        "margatroid",
        "workspace",
        "up",
        "demo",
        "--backend",
        "ws://localhost:4000/ws"
    ])
    .is_ok());
    for verb in ["catalog", "refresh", "reload", "create", "rename", "delete"] {
        assert!(Cli::try_parse_from(["margatroid", "workspace", verb]).is_err());
    }
}
#[test]
fn timestamps_and_redirected_logs_keep_real_time_without_ansi() {
    assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(
        format_timestamp(1_786_281_543_032),
        "2026-08-09T13:19:03.032Z"
    );
    assert_eq!(
        format_log_line(0, "INFO", "margatroid_cli", "started", "", false),
        "1970-01-01T00:00:00Z INFO  margatroid_cli: started"
    );
    assert!(format_log_line(0, "WARN", "cli", "test", "", true).contains("\x1b[33m"));
}
