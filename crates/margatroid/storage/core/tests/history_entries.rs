use rusqlite::Connection;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use storage_core::*;

fn record(key: &str) -> HistoryRecord {
    HistoryRecord {
        source: None,
        turn: Some("turn".into()),
        kind: "message".into(),
        content: key.into(),
        payload: json!({"type":"user","content":key}),
        created_at_ms: None,
    }
}
fn entries(storage: &AgentStorage) -> Vec<StoredHistoryRecord> {
    let StorageValue::HistoryEntries(entries) =
        execute_storage(storage, StorageOperation::ReadHistoryEntries).unwrap()
    else {
        panic!()
    };
    entries
}
#[test]
fn actual_sequences_and_timestamps_survive_duplicate_appends_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let settings = directory.path().join("settings.toml");
    let storage = open_storage(&path, &settings, "").unwrap();
    Connection::open(&path).unwrap().execute("INSERT INTO mcl_records(sequence,source,turn,content,payload,created_at_ms) VALUES(41,'original-source',NULL,'stored separately',?1,0)",[json!({"text":"keep","untouched":true}).to_string()]).unwrap();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let fresh = record("fresh");
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![fresh.clone(), fresh.clone()],
        },
    )
    .unwrap();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let saved = entries(&storage);
    assert_eq!(
        saved.iter().map(|entry| entry.sequence).collect::<Vec<_>>(),
        vec![41, 42, 43]
    );
    assert_eq!(saved[0].record.created_at_ms, Some(0));
    assert_eq!(saved[0].record.source.as_deref(), Some("original-source"));
    assert_eq!(saved[0].record.content, "stored separately");
    assert!((before..=after).contains(&saved[1].record.created_at_ms.unwrap()));
    let mut retry = fresh;
    retry.created_at_ms = Some(7);
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![retry],
        },
    )
    .unwrap();
    let appended = entries(&storage);
    assert_eq!(&appended[..3], saved.as_slice());
    assert_eq!(appended[3].sequence, 44);
    assert_eq!(appended[3].record.created_at_ms, Some(7));
    let saved = appended;
    drop(storage);
    let reopened = open_storage(&path, &settings, "different='default'").unwrap();
    assert_eq!(entries(&reopened), saved);
    let StorageValue::History(history) =
        execute_storage(&reopened, StorageOperation::ReadHistory).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        history,
        saved
            .into_iter()
            .map(|entry| entry.record)
            .collect::<Vec<_>>()
    );
    execute_storage(
        &reopened,
        StorageOperation::AppendHistory {
            records: vec![record("next")],
        },
    )
    .unwrap();
    assert_eq!(entries(&reopened).last().unwrap().sequence, 45);
}

#[test]
fn mixed_history_streams_share_sequences_and_atomic_batch_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    let mut audit = record("audit");
    audit.kind = "mcl".into();
    audit.source = Some("original-source".into());
    audit.content = "run".into();
    audit.payload =
        json!({"text":"run","arguments":{"nested":[1,null]},"error":null,"id":"request"});
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![record("first"), audit.clone(), record("last")],
        },
    )
    .unwrap();
    let original = entries(&storage);
    assert_eq!(
        original
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        execute_storage(&storage, StorageOperation::ReadAgentMessages).unwrap(),
        StorageValue::HistoryEntries(vec![original[0].clone(), original[2].clone()])
    );
    assert_eq!(
        execute_storage(&storage, StorageOperation::ReadMclRecords).unwrap(),
        StorageValue::HistoryEntries(vec![original[1].clone()])
    );
    for (kind, payload) in [
        ("event", json!({"type":"user","content":"bad"})),
        ("message", json!({"content":"bad"})),
        ("mcl", json!({"text":7})),
    ] {
        let mut invalid = record("bad");
        invalid.kind = kind.into();
        invalid.payload = payload;
        invalid.content = "bad".into();
        assert_eq!(
            execute_storage(
                &storage,
                StorageOperation::AppendHistory {
                    records: vec![audit.clone(), record("new"), invalid]
                }
            )
            .unwrap_err()
            .kind,
            StorageErrorKind::InvalidRecord
        );
        assert_eq!(entries(&storage), original);
    }
    let mut error = record("error");
    error.payload = json!({"type":"error","message":"error"});
    let mut inject = record("inject");
    inject.content = String::new();
    inject.payload = json!({"type":"inject","messages":[{"type":"user","content":"nested"}]});
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![error, inject],
        },
    )
    .unwrap();
    let saved = entries(&storage);
    assert_eq!(saved[3].sequence, 4);
    assert_eq!(saved[3].record.content, "error");
    assert_eq!(
        saved[4].record.content,
        r#"[{"content":"nested","type":"user"}]"#
    );
    let database = Connection::open(path).unwrap();
    for (table, expected) in [
        (
            "agent_messages",
            vec![
                "sequence",
                "turn",
                "role",
                "content",
                "data",
                "tool_call",
                "payload",
                "created_at_ms",
            ],
        ),
        (
            "mcl_records",
            vec![
                "sequence",
                "source",
                "turn",
                "content",
                "payload",
                "created_at_ms",
            ],
        ),
    ] {
        let columns = database
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(columns, expected);
    }
}

#[test]
fn normalized_message_content_and_remaining_payload_preserve_source_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    let source = " agent:原始 地址/<caller>?x=1 ";
    let mut message = record("payload content");
    message.source = None;
    message.content = "调用方正文\n原样".into();
    message.payload["key"] = json!("preserved application key");
    message.payload["idempotency"] = json!("preserved payload data");
    message.created_at_ms = Some(3);
    let mut mcl = message.clone();
    mcl.kind = "mcl".into();
    mcl.source = Some(source.into());
    mcl.content = "原始命令显示".into();
    mcl.payload = json!({"text":"different command", "extra":{"keep":true}});
    for _ in 0..2 {
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![message.clone(), mcl.clone()],
            },
        )
        .unwrap();
    }
    let saved = entries(&storage);
    assert_eq!(
        saved.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    for (index, entry) in saved.iter().enumerate() {
        if index % 2 == 0 {
            let mut expected = message.clone();
            expected.content = "payload content".into();
            expected.payload["sequence"] = json!(entry.sequence);
            assert_eq!(entry.record, expected);
        } else {
            assert_eq!(entry.record, mcl);
        }
    }
    let db = Connection::open(path).unwrap();
    let raw: (String, String, String) = db
        .query_row(
            "SELECT role,content,payload FROM agent_messages WHERE sequence=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(raw.0, "user");
    assert_eq!(saved[1].record.source.as_deref(), Some(source));
    assert_eq!(raw.1, "payload content");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw.2).unwrap(),
        json!({"key":"preserved application key","idempotency":"preserved payload data"})
    );
}

#[test]
fn invalid_sources_and_sql_failures_rollback_whole_mixed_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    for source in [
        "",
        "bad\nsource",
        "bad\0source",
        "bad\u{7f}source",
        "bad\u{85}source",
    ] {
        let mut invalid = record("bad");
        invalid.kind = "mcl".into();
        invalid.payload = json!({"text":"bad"});
        invalid.source = Some(source.into());
        assert_eq!(
            execute_storage(
                &storage,
                StorageOperation::AppendHistory {
                    records: vec![record("first"), invalid]
                }
            )
            .unwrap_err()
            .kind,
            StorageErrorKind::InvalidRecord
        );
        assert!(entries(&storage).is_empty());
    }
    Connection::open(path).unwrap().execute_batch("CREATE TRIGGER fail_mcl BEFORE INSERT ON mcl_records BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
    let mut mcl = record("mcl");
    mcl.kind = "mcl".into();
    mcl.source = Some("caller".into());
    mcl.payload = json!({"text":"run"});
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![record("first"), mcl]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::WriteFailed
    );
    assert!(entries(&storage).is_empty());
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![record("after rollback")],
        },
    )
    .unwrap();
    assert_eq!(entries(&storage)[0].sequence, 1);
}

#[test]
fn message_roles_are_stored_in_role_column_and_invalid_roles_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    for message_type in ["user", "assistant", "error"] {
        let mut message = record("independent content");
        message.payload = if message_type == "error" {
            json!({"type":"error","message":"payload content"})
        } else {
            json!({"type": message_type, "content": "payload content"})
        };
        execute_storage(
            &storage,
            StorageOperation::AppendHistory {
                records: vec![message],
            },
        )
        .unwrap();
    }
    let db = Connection::open(path).unwrap();
    let types = db
        .prepare("SELECT role FROM agent_messages ORDER BY sequence")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(types, ["user", "assistant", "error"]);
    assert!(entries(&storage)
        .iter()
        .all(|entry| entry.record.source.is_none()));
    db.execute(
        "UPDATE agent_messages SET role='unknown' WHERE sequence=1",
        [],
    )
    .unwrap();
    assert_eq!(
        execute_storage(&storage, StorageOperation::ReadHistoryEntries)
            .unwrap_err()
            .kind,
        StorageErrorKind::SchemaMismatch
    );
}
