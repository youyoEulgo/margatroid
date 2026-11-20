use std::{fs, path::PathBuf};
use workspace_core::*;

const LEGACY:&str="project_root: /projects/margatroid\nagents:\n  coder:\n    driver: margatroid/coder:dev\n  reviewer:\n    driver: margatroid/reviewer:dev\n";
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
    assert_eq!(definition.members[0].driver, "driver:margatroid/coder:dev");
    assert_eq!(
        definition.members[1].driver,
        "driver:margatroid/reviewer:dev"
    );
    let home = PathBuf::from(std::env::var_os("HOME").expect("legacy default requires HOME"));
    assert!(home.is_absolute());
    for member in definition.members {
        assert_eq!(member.home_root, home);
    }
}
#[test]
fn legacy_relative_home_uses_project_root_and_new_members_keep_existing_relative_base() {
    let legacy=load("project_root: /projects/demo\nagents:\n  coder: {driver: driver:margatroid/coder:dev, home_root: homes/coder}\n  reviewer: {driver: margatroid/reviewer:v2, home_root: /explicit/home}\n").unwrap();
    assert_eq!(
        legacy.members[0].home_root,
        PathBuf::from("/projects/demo/homes/coder")
    );
    assert_eq!(legacy.members[1].home_root, PathBuf::from("/explicit/home"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.yaml");
    let source="project_root: project\nmembers:\n  - {name: coder, driver: margatroid/coder:dev, home_root: homes/coder}\n  - {name: reviewer, driver: driver:margatroid/reviewer:dev}\n";
    fs::write(&path, source).unwrap();
    let members = load_workspace_definition(&path).unwrap();
    assert_eq!(
        members.members[0].home_root,
        directory.path().join("homes/coder")
    );
    assert_eq!(members.members[0].driver, "driver:margatroid/coder:dev");
    assert_eq!(members.members[1].driver, "driver:margatroid/reviewer:dev");
    assert_eq!(fs::read_to_string(path).unwrap(), source);
}
#[test]
fn both_forms_duplicate_keys_and_unknown_fields_are_refused_instead_of_overwritten() {
    for source in [
        "project_root: /project\nagents: {}\nmembers: []\n",
        "project_root: /project\nagents: null\nmembers: []\n",
        "project_root: /project\nagents:\n  coder: {driver: margatroid/one:dev}\n  coder: {driver: margatroid/two:dev}\n",
        "name: other\nproject_root: /project\nagents: {}\n",
        "project_root: /project\nagents:\n  coder:\n    driver: margatroid/one:dev\n    driver: margatroid/two:dev\n",
        "project_root: /project\nagents: {}\nagents: {}\n",
        "project_root: /project\nmembers: []\nmembers: []\n",
        "project_root: /project\nagents: {coder: {name: reviewer, driver: margatroid/coder:dev}}\n",
        "project_root: /project\nmembers: [{name: coder, driver: margatroid/one:dev},{name: coder, driver: margatroid/two:dev}]\n",
        "project_root: /project\nmembers: [{driver: margatroid/coder:dev}]\n",
        "project_root: /project\nagents: {coder: {driver: margatroid/coder:dev, database_path: /arbitrary/state.sqlite}}\n",
        "project_root: /project\nunknown: true\nagents: {}\n",
        "project_root: /project\nagents: {coder: {driver: margatroid/coder:dev, home_root: ''}}\n",
    ] {assert_eq!(load(source).unwrap_err().kind,WorkspaceErrorKind::InvalidDefinition,"accepted: {source}");}
}
#[test]
fn driver_inputs_are_only_normalized_locators_not_absolute_paths_or_urls() {
    for driver in [
        "/tmp/driver.toml",
        "https://example.com/driver.toml",
        "file:///tmp/driver.toml",
        "tool:margatroid/coder:dev",
        "margatroid/../coder",
        "margatroid/coder/driver.toml",
        "margatroid/coder:",
        "../coder",
        "margatroid/coder:tag:extra",
        "C:\\drivers\\coder",
        "margatroid/coder with spaces",
    ] {
        let source = format!("project_root: /project\nagents:\n  coder:\n    driver: '{driver}'\n");
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
    let broken = "project_root: /project\nagents: {coder: {driver: https://outside/driver.toml}}\n";
    fs::write(&path, broken).unwrap();
    assert_eq!(
        load_workspace_definition(&path).unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(cache.get(&path).unwrap(), good);
    assert_eq!(fs::read_to_string(path).unwrap(), broken);
}

#[test]
fn driver_locator_requires_an_explicit_nonempty_tag() {
    for driver in [
        "driver:margatroid/coder",
        "margatroid/coder",
        "driver:margatroid/coder:",
    ] {
        assert!(load(&format!("project_root: /project\nmembers: [{{name: coder, driver: '{driver}', home_root: /project}}]\n")).is_err(),"{driver}");
    }
    assert!(load("project_root: /project\nmembers: [{name: coder, driver: 'driver:margatroid/coder:dev', home_root: /project}]\n").is_ok());
}
