#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn interruptions_roll_back_precommit_and_complete_committed_idempotently() {
        for member in [false, true] {
            for point in [
                "preparing",
                "database",
                "prepared",
                "committed",
                "backup",
                "member_directory",
                "published",
            ] {
                if !member && point == "member_directory" {
                    continue;
                }
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("workspaces");
                let store = FileWorkspaceCatalogStore::new(&root).unwrap();
                let yaml = format!("name: old\nproject_root: {}\nagents: {{a: {{driver: local/test}}, b: {{driver: local/test}}}}\n", temp.path().display());
                let definition = compile_str(&yaml, root.join("old/workspace.yaml")).unwrap();
                let saved = store.save(&definition, "").unwrap();
                for agent in &definition.agents {
                    drop(memory_plugin::SqliteAgentDatabase::open(&agent.history_path).unwrap());
                }
                let db = rusqlite::Connection::open(&definition.agents[0].history_path).unwrap();
                db.execute("INSERT INTO block_definitions VALUES (71,'agent:old/a:latest','original-instance','temp',1,'{}')", []).unwrap();
                db.execute(
                    "INSERT INTO block_fields VALUES (71,'messages','MESSAGE','[]')",
                    [],
                )
                .unwrap();
                db.execute("INSERT INTO history_records VALUES (17,'key','agent:old/a:latest',NULL,'audit','unaltered','{}',2345)", []).unwrap();
                drop(db);
                let before = digest_tree(&root.join("old")).unwrap();
                FAULT.with(|fault| fault.set(Some(point)));
                let result = if member {
                    store.rename_member(
                        &definition.id,
                        &saved.revision,
                        &definition.agents[0].id,
                        "z",
                    )
                } else {
                    store.rename_workspace(&definition.id, &saved.revision, "new", &definition)
                };
                assert!(result.is_err(), "{point}");
                let committed = matches!(
                    point,
                    "committed" | "backup" | "member_directory" | "published"
                );
                if !committed {
                    assert_eq!(digest_tree(&root.join("old")).unwrap(), before);
                }
                drop(store);
                for _ in 0..2 {
                    let recovered = FileWorkspaceCatalogStore::new(&root).unwrap();
                    let entries = recovered.list().unwrap();
                    assert_eq!(entries.len(), 1, "{point}");
                    assert!(
                        entries[0].error.is_none(),
                        "{point}: {:?}",
                        entries[0].error
                    );
                    assert_eq!(
                        entries[0].name,
                        if committed && !member { "new" } else { "old" }
                    );
                    let db = rusqlite::Connection::open(
                        &entries[0].definition.as_ref().unwrap().agents[0].history_path,
                    )
                    .unwrap();
                    let actual: (i64, String, String) = db
                        .query_row("SELECT id,agent,instance FROM block_definitions", [], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                        })
                        .unwrap();
                    assert_eq!(
                        actual,
                        (
                            71,
                            format!(
                                "agent:{}/{}:latest",
                                entries[0].name,
                                if member && committed { "z" } else { "a" }
                            ),
                            "original-instance".into()
                        )
                    );
                    assert_eq!(
                        db.query_row(
                            "SELECT source FROM history_records WHERE sequence=17",
                            [],
                            |r| r.get::<_, String>(0)
                        )
                        .unwrap(),
                        "agent:old/a:latest"
                    );
                }
            }
        }
    }

    #[test]
    fn delete_interruptions_keep_intact_home_and_recover_idempotently() {
        for point in ["delete_prepared", "delete_committed", "delete_moved"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("workspaces");
            let store = FileWorkspaceCatalogStore::new(&root).unwrap();
            let yaml = format!(
                "name: old\nproject_root: {}\nagents: []\n",
                temp.path().display()
            );
            let definition = compile_str(&yaml, root.join("old/workspace.yaml")).unwrap();
            let saved = store.save(&definition, "").unwrap();
            fs::write(root.join("old/retained"), b"unchanged").unwrap();
            let before = digest_tree(&root.join("old")).unwrap();
            FAULT.with(|fault| fault.set(Some(point)));
            assert!(store
                .delete_workspace(&definition.id, &saved.revision)
                .is_err());
            for _ in 0..2 {
                let recovered = FileWorkspaceCatalogStore::new(&root).unwrap();
                if point == "delete_prepared" {
                    assert_eq!(recovered.list().unwrap().len(), 1);
                    assert_eq!(digest_tree(&root.join("old")).unwrap(), before);
                } else {
                    assert!(recovered.list().unwrap().is_empty());
                    let trash = fs::read_dir(root.join(".trash"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                    assert_eq!(digest_tree(&trash).unwrap(), before);
                }
            }
        }
    }

    #[test]
    fn malformed_journal_never_follows_paths_and_blocks_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspaces");
        let store = FileWorkspaceCatalogStore::new(&root).unwrap();
        let yaml = format!(
            "name: safe\nproject_root: {}\nagents: []\n",
            temp.path().display()
        );
        let definition = compile_str(&yaml, root.join("safe/workspace.yaml")).unwrap();
        let saved = store.save(&definition, "").unwrap();
        let original = fs::read(&saved.path).unwrap();
        let op = root.join(".operations/op-invalid");
        fs::create_dir_all(&op).unwrap();
        fs::write(
            op.join("journal.yaml"),
            "version: 1\nsource: ../../outside\nphase: committed\n",
        )
        .unwrap();
        let recovered = FileWorkspaceCatalogStore::new(&root).unwrap();
        assert!(recovered.list().unwrap()[0]
            .error
            .as_ref()
            .unwrap()
            .contains("recovery_required"));
        assert!(recovered.load(&definition.id, &saved.revision).is_err());
        assert!(recovered
            .delete_workspace(&definition.id, &saved.revision)
            .is_err());
        assert_eq!(fs::read(saved.path).unwrap(), original);
    }
}
