use core_plugin::World;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use workspace_core::*;

fn definition(name: &str) -> WorkspaceDefinition {
    WorkspaceDefinition {
        name: name.into(),
        project_root: "project".into(),
        members: vec![WorkspaceMemberDefinition {
            name: "coder".into(),
            driver: "drivers/coder".into(),
            home_root: "home/coder".into(),
        }],
    }
}
#[test]
fn cloned_cache_shares_arc_and_exposes_cross_thread_updates_as_owned_snapshots() {
    let definitions = WorkspaceDefinitions::default();
    let copy = definitions.clone();
    assert!(Arc::ptr_eq(&definitions.entries, &copy.entries));
    let path = PathBuf::from("workspace.yaml");
    let other = path.clone();
    std::thread::spawn(move || {
        copy.entries
            .write()
            .unwrap()
            .insert(other, definition("initial"));
    })
    .join()
    .unwrap();
    let snapshot = definitions.get(&path).unwrap();
    definitions
        .entries
        .write()
        .unwrap()
        .get_mut(&path)
        .unwrap()
        .name = "updated".into();
    assert_eq!(snapshot.name, "initial");
    assert_eq!(definitions.get(&path).unwrap().name, "updated");
    assert_eq!(definitions.list().len(), 1);
    let mut world = World::new();
    world.insert_resource(definitions);
    assert_eq!(
        world.workspace_definitions().unwrap().list()[0].1.name,
        "updated"
    );
}
#[test]
fn fresh_load_reads_current_file_and_never_uses_or_mutates_display_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.yaml");
    let cache = WorkspaceDefinitions::default();
    cache
        .entries
        .write()
        .unwrap()
        .insert(path.clone(), definition("cached"));
    std::fs::write(&path,"name: first\nproject_root: project\nmembers:\n  - name: coder\n    driver: drivers/coder\n    home_root: home/coder\n").unwrap();
    let loaded = load_workspace_definition(&path).unwrap();
    assert_eq!(loaded.name, "first");
    assert_eq!(loaded.project_root, dir.path().join("project"));
    assert_eq!(loaded.members[0].home_root, dir.path().join("home/coder"));
    assert_eq!(loaded.members[0].driver, "driver:drivers/coder");
    std::fs::write(&path, "name: second\nproject_root: next\nmembers: []\n").unwrap();
    assert_eq!(load_workspace_definition(&path).unwrap().name, "second");
    assert_eq!(cache.get(&path).unwrap().name, "cached");
    std::fs::write(&path, "broken: [").unwrap();
    assert_eq!(
        load_workspace_definition(&path).unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(cache.get(&path).unwrap().name, "cached");
    std::fs::remove_file(&path).unwrap();
    assert!(load_workspace_definition(&path).is_err());
    assert_eq!(cache.get(&path).unwrap().name, "cached");
}
#[test]
fn definitions_reject_oversize_unknown_duplicate_and_invalid_member_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.yaml");
    for text in ["name: x\nproject_root: p\nunknown: 1\n", "name: x\nproject_root: p\nmembers:\n- {name: one, driver: d, home_root: h}\n- {name: one, driver: e, home_root: j}\n", "name: ''\nproject_root: p\n", "name: x\nproject_root: p\nmembers:\n- {name: one, driver: '', home_root: h}\n"] {
        std::fs::write(&path,text).unwrap();assert_eq!(load_workspace_definition(&path).unwrap_err().kind,WorkspaceErrorKind::InvalidDefinition);
    }
    std::fs::write(&path, vec![b' '; MAX_WORKSPACE_DEFINITION_BYTES + 1]).unwrap();
    assert!(load_workspace_definition(&path)
        .unwrap_err()
        .message
        .contains("1 MiB"));
    let mut excessive = definition("many");
    excessive.members = vec![excessive.members[0].clone(); MAX_WORKSPACE_MEMBERS + 1];
    assert_eq!(
        validate_workspace_definition(&excessive).unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
}
#[test]
fn entity_accessors_keep_full_generation_and_domain_independent_members() {
    let mut world = World::new();
    assert_eq!(
        world.workspace_definitions().unwrap_err().kind,
        WorkspaceErrorKind::Missing
    );
    let workspace = world.spawn();
    world.insert_component(
        workspace,
        Workspace {
            name: "w".into(),
            definition_path: "w.yaml".into(),
            project_root: "project".into(),
            state: WorkspaceState::Starting,
            agents: BTreeSet::new(),
        },
    );
    let first = world.spawn();
    let stale = world.spawn();
    world.despawn(stale);
    let second = world.spawn();
    assert_eq!(stale.index(), second.index());
    assert_ne!(stale.generation(), second.generation());
    world.workspace_insert_agent(workspace, first).unwrap();
    world.workspace_insert_agent(workspace, second).unwrap();
    world.workspace_insert_agent(workspace, first).unwrap();
    assert_eq!(world.workspace_agents(workspace).unwrap().len(), 2);
    assert_eq!(
        world
            .workspace_insert_agent(workspace, stale)
            .unwrap_err()
            .kind,
        WorkspaceErrorKind::Missing
    );
    for state in [
        WorkspaceState::Starting,
        WorkspaceState::Running,
        WorkspaceState::Restarting,
        WorkspaceState::Stopping,
    ] {
        world
            .get_component_mut::<Workspace>(workspace)
            .unwrap()
            .state = state;
        assert_eq!(*world.workspace_state(workspace).unwrap(), state);
    }
    world.workspace_remove_agent(workspace, stale).unwrap();
    assert_eq!(world.workspace_agents(workspace).unwrap().len(), 2);
    world.workspace_remove_agent(workspace, first).unwrap();
    assert_eq!(world.workspace_agents(workspace).unwrap(), vec![second]);
    world.despawn(workspace);
    assert_eq!(
        world.workspace_state(workspace).unwrap_err().kind,
        WorkspaceErrorKind::Missing
    );
}
#[test]
fn lifecycle_contracts_are_path_based_and_self_contained() {
    let mut world = World::new();
    let workspace = world.spawn();
    let path = PathBuf::from("new.yaml");
    let start = StartWorkspace {
        id: "start".into(),
        path: path.clone(),
    };
    let restart = RestartWorkspace {
        id: "restart".into(),
        path,
    };
    let stop = StopWorkspace {
        id: "stop".into(),
        workspace,
    };
    world.emit_event(start.clone());
    world.emit_event(restart.clone());
    world.emit_event(stop.clone());
    world.advance_event_frame();
    assert_eq!(
        world
            .event_reader::<StartWorkspace>()
            .into_iter()
            .next()
            .unwrap(),
        &start
    );
    assert_eq!(
        world
            .event_reader::<RestartWorkspace>()
            .into_iter()
            .next()
            .unwrap(),
        &restart
    );
    assert_eq!(
        world
            .event_reader::<StopWorkspace>()
            .into_iter()
            .next()
            .unwrap(),
        &stop
    );
    let output = WorkspaceOperationOutput {
        workspace,
        members: vec![],
        member_errors: vec![MemberDiagnostic {
            name: "coder".into(),
            error: WorkspaceError::new(WorkspaceErrorKind::MemberFailed, "failed"),
        }],
    };
    let result = StartWorkspaceResult {
        id: "start".into(),
        result: Ok(output.clone()),
    };
    world.emit_event(result.clone());
    world.advance_event_frame();
    assert_eq!(
        world
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .next()
            .unwrap(),
        &result
    );
    assert_eq!(result.result.unwrap(), output);
}
