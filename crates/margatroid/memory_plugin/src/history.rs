use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use margatroid_types::history_components::{AgentHistoryStore, HistoryError, HistoryRecord};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_records (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE CHECK(length(key) > 0),
    source TEXT NOT NULL,
    turn TEXT,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    payload TEXT NOT NULL
);
"#;

#[derive(Clone)]
pub struct SqliteHistoryStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteHistoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(write_failed)?;
        }
        let connection = Connection::open(path).map_err(write_failed)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(write_failed)?;
        connection.execute_batch(SCHEMA).map_err(write_failed)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, HistoryError> {
        self.connection
            .lock()
            .map_err(|_| HistoryError::WriteFailed("history connection mutex poisoned".into()))
    }
}

impl AgentHistoryStore for SqliteHistoryStore {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        for record in &records {
            if record.key.is_empty() {
                return Err(HistoryError::InvalidRecord(
                    "history key must not be empty".into(),
                ));
            }
        }
        if records.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(write_failed)?;
        for record in records {
            let existing = transaction
                .query_row(
                    "SELECT key, source, turn, kind, content, payload FROM history_records WHERE key = ?1",
                    params![record.key],
                    read_row,
                )
                .optional()
                .map_err(write_failed)?
                .map(decode_record)
                .transpose()?;
            if let Some(existing) = existing {
                if existing != record {
                    return Err(HistoryError::KeyConflict(record.key));
                }
                continue;
            }
            let payload = serde_json::to_string(&record.payload).map_err(write_failed)?;
            transaction
                .execute(
                    "INSERT INTO history_records (key, source, turn, kind, content, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![record.key, record.source, record.turn, record.kind, record.content, payload],
                )
                .map_err(write_failed)?;
        }
        transaction.commit().map_err(write_failed)
    }

    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare("SELECT key, source, turn, kind, content, payload FROM history_records ORDER BY sequence ASC")
            .map_err(write_failed)?;
        let rows = statement.query_map([], read_row).map_err(write_failed)?;
        rows.map(|row| decode_record(row.map_err(write_failed)?))
            .collect()
    }
}

type StoredRecord = (String, String, Option<String>, String, String, String);

fn read_row(row: &Row<'_>) -> rusqlite::Result<StoredRecord> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn decode_record(record: StoredRecord) -> Result<HistoryRecord, HistoryError> {
    let (key, source, turn, kind, content, payload) = record;
    Ok(HistoryRecord {
        key,
        source,
        turn,
        kind,
        content,
        payload: serde_json::from_str(&payload).map_err(write_failed)?,
    })
}

fn write_failed(error: impl std::fmt::Display) -> HistoryError {
    HistoryError::WriteFailed(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use margatroid_types::history_components::AgentHistoryHandle;
    use serde_json::json;
    use tempfile::tempdir;

    fn record(key: &str) -> HistoryRecord {
        HistoryRecord {
            key: key.into(),
            source: "test".into(),
            turn: Some("turn-1".into()),
            kind: "message".into(),
            content: format!("content-{key}"),
            payload: json!({"message": {"text": "你好", "items": [1, true, null]}}),
        }
    }

    #[test]
    fn reopen_preserves_all_fields_and_only_creates_history_table() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("nested/history.sqlite");
        let first = record("first");
        let mut second = record("second");
        second.turn = None;
        {
            let store = SqliteHistoryStore::open(&path).unwrap();
            store
                .append_batch(vec![first.clone(), second.clone()])
                .unwrap();
        }
        let restored = SqliteHistoryStore::open(&path).unwrap();
        assert_eq!(restored.list().unwrap(), vec![first, second]);
        let connection = restored.lock_connection().unwrap();
        let tables: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(tables, vec!["history_records"]);
    }

    #[test]
    fn identical_keys_are_idempotent_within_and_across_batches() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let first = record("first");
        store
            .append_batch(vec![first.clone(), first.clone()])
            .unwrap();
        store.append_batch(vec![first.clone()]).unwrap();
        assert_eq!(store.list().unwrap(), vec![first]);
    }

    #[test]
    fn payload_comparison_ignores_json_object_serialization_order() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let mut first = record("first");
        first.payload = json!({"a": 1, "b": {"x": 2, "y": 3}});
        store.append_batch(vec![first.clone()]).unwrap();
        store
            .lock_connection()
            .unwrap()
            .execute(
                "UPDATE history_records SET payload = ?1 WHERE key = ?2",
                params![r#"{"b":{"y":3,"x":2},"a":1}"#, first.key],
            )
            .unwrap();
        store.append_batch(vec![first.clone()]).unwrap();
        assert_eq!(store.list().unwrap(), vec![first]);
    }

    #[test]
    fn conflicting_existing_key_rolls_back_the_entire_batch() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let first = record("first");
        store.append_batch(vec![first.clone()]).unwrap();
        let mut conflict = first.clone();
        conflict.content = "different".into();
        assert_eq!(
            store.append_batch(vec![record("new"), conflict]),
            Err(HistoryError::KeyConflict("first".into()))
        );
        assert_eq!(store.list().unwrap(), vec![first]);
    }

    #[test]
    fn conflicting_duplicate_in_one_batch_rolls_back() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let first = record("first");
        let mut conflict = first.clone();
        conflict.payload = json!({"different": true});
        assert_eq!(
            store.append_batch(vec![first, record("second"), conflict]),
            Err(HistoryError::KeyConflict("first".into()))
        );
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn every_field_participates_in_key_conflict_detection() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let first = record("first");
        store.append_batch(vec![first.clone()]).unwrap();
        let mut alternatives = vec![first.clone(); 5];
        alternatives[0].source = "other".into();
        alternatives[1].turn = None;
        alternatives[2].kind = "other".into();
        alternatives[3].content = "other".into();
        alternatives[4].payload = json!(null);
        for alternative in alternatives {
            assert_eq!(
                store.append_batch(vec![alternative]),
                Err(HistoryError::KeyConflict("first".into()))
            );
        }
        assert_eq!(store.list().unwrap(), vec![first]);
    }

    #[test]
    fn empty_key_rejects_whole_batch_and_empty_batch_is_valid() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        assert!(matches!(
            store.append_batch(vec![record("first"), record("")]),
            Err(HistoryError::InvalidRecord(_))
        ));
        store.append_batch(vec![]).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn sequence_is_insertion_order_and_unchanged_by_retries_or_rollback() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let first = record("z");
        let second = record("a");
        store
            .append_batch(vec![first.clone(), second.clone(), first.clone()])
            .unwrap();
        let mut conflict = second.clone();
        conflict.turn = Some("other-turn".into());
        assert!(store
            .append_batch(vec![record("rolled-back"), conflict])
            .is_err());
        let third = record("m");
        store
            .append_batch(vec![second.clone(), third.clone()])
            .unwrap();
        assert_eq!(store.list().unwrap(), vec![first, second, third]);
        let connection = store.lock_connection().unwrap();
        let sequences: Vec<i64> = connection
            .prepare("SELECT sequence FROM history_records ORDER BY sequence")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(sequences, vec![1, 2, 3]);
    }

    #[test]
    fn database_write_failure_rolls_back_the_entire_batch() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        store.lock_connection().unwrap().execute_batch(
            "CREATE TRIGGER fail_history_insert BEFORE INSERT ON history_records WHEN NEW.key = 'fail' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        ).unwrap();
        assert!(matches!(
            store.append_batch(vec![record("first"), record("fail")]),
            Err(HistoryError::WriteFailed(_))
        ));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn clones_and_trait_handles_share_a_connection_between_threads() {
        let store = SqliteHistoryStore::open(":memory:").unwrap();
        let handle = AgentHistoryHandle::new(store.clone());
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    handle
                        .append_batch(vec![record("shared"), record(&format!("thread-{index}"))])
                        .unwrap();
                    assert!(!handle.list().unwrap().is_empty());
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let records = store.list().unwrap();
        assert_eq!(records.len(), 9);
        assert_eq!(records[0].key, "shared");
        for index in 0..8 {
            assert_eq!(
                records
                    .iter()
                    .filter(|item| item.key == format!("thread-{index}"))
                    .count(),
                1
            );
        }
    }
}
