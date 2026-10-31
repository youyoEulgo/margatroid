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
                    "UPDATE storage_messages SET value_json=?1",
                    [json!([n]).to_string()],
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
fn record(key: &str) -> HistoryRecord {
    HistoryRecord {
        key: key.into(),
        source: "agent:old/identity:latest".into(),
        turn: Some("turn".into()),
        kind: "message".into(),
        content: "正文".into(),
        payload: json!({"nested":[1,true,null]}),
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
    Connection::open(&db).unwrap().execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON storage_messages BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
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
fn history_batches_are_idempotent_preserve_first_timestamp_and_rollback_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![record("a"), record("a")],
        },
    )
    .unwrap();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let saved = history(&storage);
    assert_eq!(saved.len(), 1);
    assert!((before..=after).contains(&saved[0].created_at_ms.unwrap()));
    let mut retry = record("a");
    retry.created_at_ms = Some(7);
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![retry],
        },
    )
    .unwrap();
    assert_eq!(history(&storage), saved);
    let mut conflict = record("a");
    conflict.content = "changed".into();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![record("new"), conflict]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::KeyConflict
    );
    assert_eq!(history(&storage), saved);
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
    let mut unknown = record("unknown");
    unknown.created_at_ms = Some(0);
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![unknown.clone()],
        },
    )
    .unwrap();
    assert_eq!(history(&storage)[1], unknown);
}
