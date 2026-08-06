use std::path::{Path, PathBuf};

use compose::{compile_str, ComposeErrorKind};

#[test]
fn documented_public_api_compiles_a_workspace_definition() {
    let source = r#"
name: demo
manager: manager
agents:
  manager:
    image: local/coder:latest
"#;

    let definition = compile_str(source, Path::new("/tmp/demo/margatroid-workspace.yaml")).unwrap();

    assert_eq!(definition.name, "demo");
    assert_eq!(definition.manager, "manager");
    assert_eq!(definition.project_root, PathBuf::from("/tmp/demo"));
}

#[test]
fn malformed_yaml_has_a_stable_error_kind() {
    let error = compile_str("name: [", "/tmp/demo/margatroid-workspace.yaml").unwrap_err();
    assert_eq!(error.kind(), ComposeErrorKind::FileDecode);
}
