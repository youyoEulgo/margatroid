use std::path::PathBuf;
use workspace_core::*;
fn definition(root: &std::path::Path) -> WorkspaceDefinition {
    WorkspaceDefinition {
        project_root: root.to_path_buf(),
        members: vec![],
    }
}
#[test]
fn resource_crud_preserves_project_and_rejects_active_or_unsafe_targets() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let sentinel = project.path().join("keep.txt");
    std::fs::write(&sentinel, "project").unwrap();
    let catalog = WorkspaceDefinitions::open(root.path()).unwrap();
    let address = "workspace:margatroid/project:dev";
    let expected = root.path().join("margatroid/project/dev/workspace.yaml");
    let original = definition(project.path());
    catalog
        .modify(WorkspaceModification::Create {
            address: address.into(),
            definition: original.clone(),
        })
        .unwrap();
    assert_eq!(catalog.path_for(address).unwrap(), expected);
    assert_eq!(catalog.list().len(), 1);
    assert!(catalog
        .modify(WorkspaceModification::Create {
            address: address.into(),
            definition: original.clone()
        })
        .is_err());
    catalog.reserve(&expected).unwrap();
    for operation in [
        WorkspaceModification::Update {
            address: address.into(),
            definition: original.clone(),
        },
        WorkspaceModification::Rename {
            address: address.into(),
            new_address: "workspace:margatroid/new:dev".into(),
        },
        WorkspaceModification::Delete {
            address: address.into(),
        },
    ] {
        assert_eq!(
            catalog.modify(operation).unwrap_err().kind,
            WorkspaceErrorKind::StateRejected
        );
    }
    catalog.release(&expected);
    let mut changed = original;
    changed.project_root = project.path().join("changed");
    catalog
        .modify(WorkspaceModification::Update {
            address: address.into(),
            definition: changed,
        })
        .unwrap();
    assert_eq!(
        catalog.get(&expected).unwrap().project_root,
        project.path().join("changed")
    );
    let data = expected.parent().unwrap().join("agents/coder");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("state.sqlite"), "fixture").unwrap();
    let new = "workspace:margatroid/renamed:dev";
    catalog
        .modify(WorkspaceModification::Rename {
            address: address.into(),
            new_address: new.into(),
        })
        .unwrap();
    let renamed = catalog.path_for(new).unwrap();
    assert!(!expected.exists());
    assert!(renamed
        .parent()
        .unwrap()
        .join("agents/coder/state.sqlite")
        .exists());
    assert_eq!(catalog.list()[0].0, renamed);
    catalog
        .modify(WorkspaceModification::Delete {
            address: new.into(),
        })
        .unwrap();
    assert!(catalog.list().is_empty());
    assert!(sentinel.is_file());
    for invalid in [
        "workspace:margatroid/project",
        "workspace:margatroid/project:",
        "workspace:../x:dev",
        "workspace:margatroid/../x:dev",
        "agent:margatroid/x:dev",
    ] {
        assert!(catalog.path_for(invalid).is_err());
    }
}
#[test]
fn directory_scanning_tracks_full_resource_identity_and_rejects_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let catalog = WorkspaceDefinitions::open(root.path()).unwrap();
    for address in [
        "workspace:margatroid/same:dev",
        "workspace:margatroid/same:test",
        "workspace:other/same:dev",
    ] {
        catalog
            .modify(WorkspaceModification::Create {
                address: address.into(),
                definition: definition(root.path()),
            })
            .unwrap();
    }
    assert_eq!(catalog.reload().unwrap().list().len(), 3);
    #[cfg(unix)]
    {
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(root.path().join("margatroid"), &linked).unwrap();
        assert!(catalog.reload().is_err());
        assert!(catalog.path_for("workspace:linked/same:dev").is_err());
    }
    let _ = PathBuf::new();
}
