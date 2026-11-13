use core_plugin::World;
use std::collections::BTreeSet;
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.yaml");
    std::fs::write(&path, "name: initial\nproject_root: project\nmembers: []\n").unwrap();
    let definitions = WorkspaceDefinitions::open(&path).unwrap();
    let copy = definitions.clone();
    assert!(definitions.same_instance(&copy));
    let updated_path = path.clone();
    std::thread::spawn(move || {
        std::fs::write(
            updated_path,
            "name: updated\nproject_root: project\nmembers: []\n",
        )
        .unwrap();
        copy.reload().unwrap();
    })
    .join()
    .unwrap();
    std::fs::write(&path, "name: initial\nproject_root: project\nmembers: []\n").unwrap();
    definitions.reload().unwrap();
    let snapshot = definitions.get(&path).unwrap();
    std::fs::write(&path, "name: updated\nproject_root: project\nmembers: []\n").unwrap();
    definitions.reload().unwrap();
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
    std::fs::write(&path, "name: cached\nproject_root: project\nmembers: []\n").unwrap();
    let cache = WorkspaceDefinitions::open(&path).unwrap();
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
        workspace_core::Workspace::new(
            "w".into(),
            "w.yaml".into(),
            "project".into(),
            WorkspaceState::Starting,
            BTreeSet::new(),
            None,
            crate::test_workspace_context(),
        ),
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
        update_workspace(&mut world, workspace, WorkspaceChange::State(state)).unwrap();
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
fn lifecycle_request_owns_original_oneshot() {
    let mut world = World::new();
    let workspace = world.spawn();
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    let request = StopWorkspace {
        id: "stop".into(),
        workspace,
        reply: std::sync::Arc::new(std::sync::Mutex::new(Some(sender))),
    };
    world.emit_event(request);
    world.advance_event_frame();
    let event = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(event.workspace, workspace);
    let sender = event.reply.lock().unwrap().take().unwrap();
    sender
        .send(Ok(WorkspaceOperationOutput {
            workspace,
            members: vec![],
            member_errors: vec![],
        }))
        .unwrap();
    assert_eq!(receiver.try_recv().unwrap().unwrap().workspace, workspace);
}

#[test]
fn full_reload_publishes_all_definitions_atomically_and_snapshots_remain_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, value: &str| {
        let path = dir.path().join(name).join("workspace.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("name: {value}\nproject_root: project\nmembers: []\n"),
        )
        .unwrap();
        path
    };
    let first = write("first", "old-first");
    let second = write("second", "old-second");
    let definitions = WorkspaceDefinitions::open(dir.path()).unwrap();
    let old = definitions.reload().unwrap();
    write("first", "new-first");
    write("second", "new-second");
    let current = definitions.reload().unwrap();
    assert_eq!(current.get(&first).unwrap().name, "new-first");
    assert_eq!(current.get(&second).unwrap().name, "new-second");
    assert_eq!(old.get(&first).unwrap().name, "old-first");
    assert_eq!(old.get(&second).unwrap().name, "old-second");
    std::fs::write(&second, "broken: [").unwrap();
    write("first", "unpublished");
    assert!(definitions.reload().is_err());
    assert_eq!(definitions.get(&first).unwrap().name, "new-first");
    assert_eq!(definitions.get(&second).unwrap().name, "new-second");
    assert_eq!(current.get(&first).unwrap().name, "new-first");
    std::fs::remove_file(&second).unwrap();
    let after = definitions.reload().unwrap();
    assert!(after.get(&second).is_none());
    assert_eq!(after.get(&first).unwrap().name, "unpublished");
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}
