use std::{fs, path::PathBuf};
use workspace_core::*;

const LEGACY:&str="name: margatroid\nproject_root: /projects/margatroid\nagents:\n  coder:\n    driver: local/coder:latest\n  reviewer:\n    driver: local/reviewer:latest\n";
fn load(source: &str) -> Result<WorkspaceDefinition, WorkspaceError> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.yaml");
    fs::write(&path, source).unwrap();
    let loaded = load_workspace_definition(&path);
    assert_eq!(fs::read_to_string(path).unwrap(), source);
    loaded
}
#[test]
fn existing_agents_map_and_omitted_home_root_load_without_rewriting_user_format() {
    let definition = load(LEGACY).unwrap();
    assert_eq!(definition.name, "margatroid");
    assert_eq!(
        definition.project_root,
        PathBuf::from("/projects/margatroid")
    );
    assert_eq!(
        definition
            .members
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>(),
        vec!["coder", "reviewer"]
    );
    assert_eq!(definition.members[0].driver, "driver:local/coder:latest");
    assert_eq!(definition.members[1].driver, "driver:local/reviewer:latest");
    let home = PathBuf::from(std::env::var_os("HOME").expect("legacy default requires HOME"));
    assert!(home.is_absolute());
    for member in definition.members {
        assert_eq!(member.home_root, home);
    }
}
#[test]
fn legacy_relative_home_uses_project_root_and_new_members_keep_existing_relative_base() {
    let legacy=load("name: demo\nproject_root: /projects/demo\nagents:\n  coder: {driver: driver:local/coder, home_root: homes/coder}\n  reviewer: {driver: local/reviewer:v2, home_root: /explicit/home}\n").unwrap();
    assert_eq!(
        legacy.members[0].home_root,
        PathBuf::from("/projects/demo/homes/coder")
    );
    assert_eq!(legacy.members[1].home_root, PathBuf::from("/explicit/home"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.yaml");
    let source="name: demo\nproject_root: project\nmembers:\n  - {name: coder, driver: local/coder, home_root: homes/coder}\n  - {name: reviewer, driver: driver:local/reviewer:latest}\n";
    fs::write(&path, source).unwrap();
    let members = load_workspace_definition(&path).unwrap();
    assert_eq!(
        members.members[0].home_root,
        directory.path().join("homes/coder")
    );
    assert_eq!(members.members[0].driver, "driver:local/coder");
    assert_eq!(members.members[1].driver, "driver:local/reviewer:latest");
    assert_eq!(fs::read_to_string(path).unwrap(), source);
}
#[test]
fn both_forms_duplicate_keys_and_unknown_fields_are_refused_instead_of_overwritten() {
    for source in [
        "name: demo\nproject_root: /project\nagents: {}\nmembers: []\n",
        "name: demo\nproject_root: /project\nagents: null\nmembers: []\n",
        "name: demo\nproject_root: /project\nagents:\n  coder: {driver: local/one}\n  coder: {driver: local/two}\n",
        "name: demo\nname: other\nproject_root: /project\nagents: {}\n",
        "name: demo\nproject_root: /project\nagents:\n  coder:\n    driver: local/one\n    driver: local/two\n",
        "name: demo\nproject_root: /project\nagents: {}\nagents: {}\n",
        "name: demo\nproject_root: /project\nmembers: []\nmembers: []\n",
        "name: demo\nproject_root: /project\nagents: {coder: {name: reviewer, driver: local/coder}}\n",
        "name: demo\nproject_root: /project\nmembers: [{name: coder, driver: local/one},{name: coder, driver: local/two}]\n",
        "name: demo\nproject_root: /project\nmembers: [{driver: local/coder}]\n",
        "name: demo\nproject_root: /project\nagents: {coder: {driver: local/coder, database_path: /arbitrary/state.sqlite}}\n",
        "name: demo\nproject_root: /project\nunknown: true\nagents: {}\n",
        "name: demo\nproject_root: /project\nagents: {coder: {driver: local/coder, home_root: ''}}\n",
    ] {assert_eq!(load(source).unwrap_err().kind,WorkspaceErrorKind::InvalidDefinition,"accepted: {source}");}
}
#[test]
fn driver_inputs_are_only_normalized_locators_not_absolute_paths_or_urls() {
    for driver in [
        "/tmp/driver.toml",
        "https://example.com/driver.toml",
        "file:///tmp/driver.toml",
        "tool:local/coder",
        "local/../coder",
        "local/coder/driver.toml",
        "local/coder:",
        "../coder",
        "local/coder:tag:extra",
        "C:\\drivers\\coder",
        "local/coder with spaces",
    ] {
        let source = format!(
            "name: demo\nproject_root: /project\nagents:\n  coder:\n    driver: '{driver}'\n"
        );
        assert_eq!(
            load(&source).unwrap_err().kind,
            WorkspaceErrorKind::InvalidDefinition,
            "accepted {driver}"
        );
    }
}
#[test]
fn invalid_legacy_reload_does_not_fall_back_to_shared_display_cache() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.yaml");
    fs::write(&path, LEGACY).unwrap();
    let good = load_workspace_definition(&path).unwrap();
    let cache = WorkspaceDefinitions::open(&path).unwrap();
    let broken="name: changed\nproject_root: /project\nagents: {coder: {driver: https://outside/driver.toml}}\n";
    fs::write(&path, broken).unwrap();
    assert_eq!(
        load_workspace_definition(&path).unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(cache.get(&path).unwrap(), good);
    assert_eq!(fs::read_to_string(path).unwrap(), broken);
}
