use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
use storage_core::*;

#[test]
fn multi_selector_read_has_one_snapshot_during_external_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    create(&storage, "b");
    replace(&storage, "b", vec![json!(0)]);
    let writer = Connection::open(path).unwrap();
    writer
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let thread = std::thread::spawn(move || {
        for n in 1..100 {
            writer
                .execute(
                    "UPDATE blocks SET fields_json=?1",
                    [json!({"messages":[n]}).to_string()],
                )
                .unwrap();
        }
    });
    for _ in 0..100 {
        let StorageValue::Messages(values) = execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector("b"), selector("b")],
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], values[1]);
    }
    thread.join().unwrap();
}

fn selector(block: &str) -> MessageSelector {
    MessageSelector {
        block: block.into(),
        field: "messages".into(),
        instance: None,
        selection: MessageSelection::All,
    }
}
fn create(storage: &AgentStorage, block: &str) {
    assert_eq!(
        execute_storage(
            storage,
            StorageOperation::CreateBlock {
                name: block.into(),
                fields: vec!["messages".into()],
                instance: None
            }
        )
        .unwrap(),
        StorageValue::Unit
    );
}
fn replace(storage: &AgentStorage, block: &str, values: Vec<Value>) {
    execute_storage(
        storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(values)],
            target: selector(block),
        },
    )
    .unwrap();
}
fn read(storage: &AgentStorage, block: &str) -> Vec<Value> {
    match execute_storage(
        storage,
        StorageOperation::ReadMessages {
            selectors: vec![selector(block)],
        },
    )
    .unwrap()
    {
        StorageValue::Messages(v) => v,
        _ => panic!(),
    }
}
fn record(_label: &str) -> HistoryRecord {
    HistoryRecord {
        source: "agent:old/identity:latest".into(),
        turn: Some("turn".into()),
        kind: "message".into(),
        content: "正文".into(),
        payload: json!({"type":"assistant","content":"正文","nested":[1,true,null]}),
        created_at_ms: None,
    }
}
fn history(storage: &AgentStorage) -> Vec<HistoryRecord> {
    match execute_storage(storage, StorageOperation::ReadHistory).unwrap() {
        StorageValue::History(v) => v,
        _ => panic!(),
    }
}

#[test]
fn persistent_reopen_retains_messages_and_history_and_create_only_opens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("agent/db.sqlite");
    let settings = dir.path().join("agent/settings.toml");
    let message = json!({"role":"assistant","tool_calls":[{"id":"x","arguments":{"a":1}}],"content":[null,"你好"]});
    {
        let storage = open_storage(&db, &settings, "model='initial'\n").unwrap();
        create(&storage, "conversation");
        replace(&storage, "conversation", vec![message.clone()]);
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![record("first")],
            },
        )
        .unwrap();
    }
    let storage = open_storage(&db, &settings, "model='different'\n").unwrap();
    create(&storage, "conversation");
    assert_eq!(read(&storage, "conversation"), vec![message]);
    assert_eq!(history(&storage).len(), 1);
    assert_eq!(fs::read_to_string(settings).unwrap(), "model='initial'\n");
}
#[test]
fn database_and_settings_initialization_are_independent_and_existing_config_is_not_merged() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let cfg = dir.path().join("settings.toml");
    fs::write(&cfg, "existing=7\n").unwrap();
    let storage = open_storage(&db, &cfg, "invalid = [").unwrap();
    create(&storage, "b");
    replace(&storage, "b", vec![json!(7)]);
    drop(storage);
    fs::remove_file(&cfg).unwrap();
    let storage = open_storage(&db, &cfg, "fresh=true\n").unwrap();
    assert_eq!(read(&storage, "b"), vec![json!(7)]);
    assert_eq!(fs::read_to_string(&cfg).unwrap(), "fresh=true\n");
    fs::write(&cfg, "broken=[").unwrap();
    assert_eq!(
        open_storage(&db, &cfg, "").err().unwrap().kind,
        StorageErrorKind::ConfigInvalid
    );
    assert_eq!(fs::read_to_string(cfg).unwrap(), "broken=[");
}
#[test]
fn failed_settings_initialization_does_not_reset_database_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    let cfg = dir.path().join("cfg");
    assert!(open_storage(&db, &cfg, "broken=[").is_err());
    assert!(db.exists());
    assert!(!cfg.exists());
    let storage = open_storage(&db, &cfg, "").unwrap();
    create(&storage, "saved");
}
#[test]
fn config_operations_reload_file_preserve_failures_and_share_file_lock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    let cfg = dir.path().join("settings.toml");
    let a = open_storage(&db, &cfg, "scalar=1\n").unwrap();
    let b = open_storage(&db, &cfg, "").unwrap();
    execute_storage(
        &a,
        StorageOperation::SetConfig {
            path: "driver.model.name".into(),
            value: "模型".into(),
        },
    )
    .unwrap();
    let before = fs::read(&cfg).unwrap();
    assert_eq!(
        execute_storage(
            &a,
            StorageOperation::SetConfig {
                path: "scalar.child".into(),
                value: 1.into()
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::ConfigTypeMismatch
    );
    assert_eq!(fs::read(&cfg).unwrap(), before);
    for path in ["", "a..b", "a[0]", " trailing"] {
        assert_eq!(
            execute_storage(&a, StorageOperation::DeleteConfig { path: path.into() })
                .unwrap_err()
                .kind,
            StorageErrorKind::ConfigPathInvalid
        );
    }
    fs::write(&cfg, "external='current'\n").unwrap();
    assert_eq!(
        execute_storage(&b, StorageOperation::ReadConfig).unwrap(),
        StorageValue::Config(toml::from_str("external='current'").unwrap())
    );
    execute_storage(
        &a,
        StorageOperation::DeleteConfig {
            path: "missing.child".into(),
        },
    )
    .unwrap();
    let workers: Vec<_> = (0..12)
        .map(|i| {
            let s = if i % 2 == 0 { a.clone() } else { b.clone() };
            std::thread::spawn(move || {
                execute_storage(
                    &s,
                    StorageOperation::SetConfig {
                        path: format!("values.k{i}"),
                        value: i.into(),
                    },
                )
                .unwrap()
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let StorageValue::Config(table) = execute_storage(&a, StorageOperation::ReadConfig).unwrap()
    else {
        panic!()
    };
    assert_eq!(table["values"].as_table().unwrap().len(), 12);
    execute_storage(
        &a,
        StorageOperation::DeleteConfig {
            path: "values.k0".into(),
        },
    )
    .unwrap();
    assert!(!fs::read_to_string(cfg).unwrap().contains("k0 ="));
}
#[test]
fn self_reference_sources_snapshot_range_and_sql_failure_preserve_messages() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    let storage = open_storage(&db, &dir.path().join("cfg"), "").unwrap();
    create(&storage, "b");
    replace(&storage, "b", vec![json!(1), json!(2)]);
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![
                MessageSource::Stored(selector("b")),
                MessageSource::Stored(selector("b")),
            ],
            target: selector("b"),
        },
    )
    .unwrap();
    let before = read(&storage, "b");
    assert_eq!(before, vec![json!(1), json!(2), json!(1), json!(2)]);
    for selection in [
        MessageSelection::Index(i64::MAX),
        MessageSelection::Index(i64::MIN),
        MessageSelection::Range(3, 2),
        MessageSelection::Range(-1, 2),
    ] {
        let mut target = selector("b");
        target.selection = selection;
        assert_eq!(
            execute_storage(
                &storage,
                StorageOperation::ReplaceMessages {
                    sources: vec![],
                    target
                }
            )
            .unwrap_err()
            .kind,
            StorageErrorKind::InvalidRange
        );
        assert_eq!(read(&storage, "b"), before);
    }
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReplaceMessages {
                sources: vec![
                    MessageSource::Literal(vec![json!(99)]),
                    MessageSource::Stored(selector("missing"))
                ],
                target: selector("b")
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    Connection::open(&db).unwrap().execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON blocks BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReplaceMessages {
                sources: vec![],
                target: selector("b")
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::WriteFailed
    );
    assert_eq!(read(&storage, "b"), before);
}
#[test]
fn negative_indices_and_insertion_boundaries_have_explicit_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    create(&storage, "b");
    replace(&storage, "b", vec![json!(1), json!(2)]);
    let mut s = selector("b");
    s.selection = MessageSelection::Index(-1);
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![s.clone()]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![json!(2)])
    );
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![json!(3)])],
            target: s,
        },
    )
    .unwrap();
    assert_eq!(read(&storage, "b"), vec![json!(1), json!(2), json!(3)]);
    let mut s = selector("b");
    s.selection = MessageSelection::Range(-3, -1);
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages { selectors: vec![s] }
        )
        .unwrap(),
        StorageValue::Messages(vec![json!(2), json!(3)])
    );
}
#[test]
fn temporary_namespaces_isolate_and_conflicting_lifecycle_does_not_recreate() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    for instance in ["process-a-vm", "process-b-vm"] {
        execute_storage(
            &storage,
            StorageOperation::CreateBlock {
                name: "temp".into(),
                fields: vec!["messages".into()],
                instance: Some(instance.into()),
            },
        )
        .unwrap();
    }
    let mut a = selector("temp");
    a.instance = Some("process-a-vm".into());
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![json!(1)])],
            target: a.clone(),
        },
    )
    .unwrap();
    let mut b = a;
    b.instance = Some("process-b-vm".into());
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages { selectors: vec![b] }
        )
        .unwrap(),
        StorageValue::Messages(vec![])
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::CreateBlock {
                name: "temp".into(),
                fields: vec!["messages".into()],
                instance: None
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::LifecycleMismatch
    );
}
#[test]
fn history_batches_append_duplicates_and_rollback_invalid_records() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for _ in 0..2 {
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![record("same")],
            },
        )
        .unwrap();
    }
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let saved = history(&storage);
    assert_eq!(saved.len(), 2);
    assert!(saved
        .iter()
        .all(|r| (before..=after).contains(&r.created_at_ms.unwrap())));
    let mut invalid = record("bad");
    invalid.created_at_ms = Some(u64::MAX);
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![record("new"), invalid]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::InvalidRecord
    );
    assert_eq!(history(&storage), saved);
    let mut epoch = record("epoch");
    epoch.created_at_ms = Some(0);
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![epoch.clone()],
        },
    )
    .unwrap();
    assert_eq!(history(&storage)[2], epoch);
}

#[test]
fn temporary_blocks_drop_per_instance_and_never_grow_persistent_database_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let cfg = dir.path().join("cfg");
    let storage = open_storage(&path, &cfg, "").unwrap();
    create(&storage, "persistent");
    replace(&storage, "persistent", vec![json!({"keep":[1,null]})]);
    let database = Connection::open(&path).unwrap();
    let pages: i64 = database
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let size = fs::metadata(&path).unwrap().len();
    let mut old = selector("temporary");
    for n in 0..100 {
        let instance = format!("vm-{n}");
        execute_storage(
            &storage,
            StorageOperation::CreateBlock {
                name: "temporary".into(),
                fields: vec!["messages".into()],
                instance: Some(instance.clone()),
            },
        )
        .unwrap();
        let mut current = selector("temporary");
        current.instance = Some(instance.clone());
        execute_storage(
            &storage,
            StorageOperation::ReplaceMessages {
                sources: vec![MessageSource::Literal(vec![json!(n)])],
                target: current.clone(),
            },
        )
        .unwrap();
        if n > 0 {
            assert_eq!(
                execute_storage(
                    &storage,
                    StorageOperation::ReadMessages {
                        selectors: vec![old]
                    }
                )
                .unwrap_err()
                .kind,
                StorageErrorKind::BlockMissing
            );
        }
        execute_storage(&storage, StorageOperation::DropTemporaryBlocks { instance }).unwrap();
        old = current;
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM blocks", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .query_row("PRAGMA page_count", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            pages
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), size);
    }
    assert_eq!(read(&storage, "persistent"), vec![json!({"keep":[1,null]})]);
    execute_storage(
        &storage,
        StorageOperation::CreateBlock {
            name: "orphan".into(),
            fields: vec!["messages".into()],
            instance: Some("crashed".into()),
        },
    )
    .unwrap();
    let mut orphan = selector("orphan");
    orphan.instance = Some("crashed".into());
    drop(storage);
    let storage = open_storage(&path, &cfg, "").unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![orphan]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
}

#[test]
fn dropping_one_temp_instance_preserves_other_instances_and_persistent_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    create(&storage, "persistent");
    for instance in ["a", "b"] {
        execute_storage(
            &storage,
            StorageOperation::CreateBlock {
                name: "temporary".into(),
                fields: vec!["messages".into()],
                instance: Some(instance.into()),
            },
        )
        .unwrap();
    }
    execute_storage(
        &storage,
        StorageOperation::DropTemporaryBlocks {
            instance: "a".into(),
        },
    )
    .unwrap();
    let mut a = selector("temporary");
    a.instance = Some("a".into());
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages { selectors: vec![a] }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    let mut b = selector("temporary");
    b.instance = Some("b".into());
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![b, selector("persistent")]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![])
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::DropTemporaryBlocks {
                instance: "".into()
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::InvalidRequest
    );
}
