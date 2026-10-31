use rusqlite::Connection;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use storage_core::*;

fn record(key: &str) -> HistoryRecord {
    HistoryRecord {
        key: key.into(),
        source: "original-source".into(),
        turn: Some("turn".into()),
        kind: "message".into(),
        content: key.into(),
        payload: json!({"key":key}),
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
fn actual_sequences_and_timestamps_survive_idempotent_retries_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let settings = directory.path().join("settings.toml");
    let storage = open_storage(&path, &settings, "").unwrap();
    Connection::open(&path).unwrap().execute("INSERT INTO history_records(sequence,key,source,turn,kind,content,payload,created_at_ms) VALUES(41,'legacy','legacy-source',NULL,'audit','keep',?1,0)",[json!({"untouched":true}).to_string()]).unwrap();
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
        vec![41, 42]
    );
    assert_eq!(saved[0].record.created_at_ms, Some(0));
    assert_eq!(saved[0].record.source, "legacy-source");
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
    assert_eq!(entries(&storage), saved);
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
    assert_eq!(entries(&reopened).last().unwrap().sequence, 43);
}
