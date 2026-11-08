use rusqlite::Connection;
use serde_json::json;
use storage_core::*;

fn record() -> AgentLogRecord {
    AgentLogRecord {
        created_at_ms: None,
        level: AgentLogLevel::Error,
        source: AgentLogSource::Tool,
        stage: "execute".into(),
        message: "failed\nwith unicode 错误".into(),
        details_json: Some(json!({"nested":{"status":false}})),
        context_json: Some(
            json!({"turn":"turn-1","task":"task-1","call":"call-1","alias":"shell","driver_instance":"driver-1"}),
        ),
        origin: Some("tool:test".into()),
        raw_error: Some(format!("original error\n{}\0end", "全文".repeat(5000))),
        traceback: Some("stack traceback:\n\ttool.lua:1\n\t[C]: in function 'error'".into()),
    }
}

#[test]
fn logs_preserve_full_record_and_context_without_entering_history() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    let config = dir.path().join("cfg");
    let storage = open_storage(&db_path, &config, "").unwrap();
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let saved = append_agent_log(&storage, record()).unwrap();
    assert_eq!(saved.sequence, 1);
    assert!(saved.record.created_at_ms.unwrap() >= start);
    let mut expected = record();
    expected.created_at_ms = saved.record.created_at_ms;
    assert_eq!(saved.record, expected);
    assert_eq!(
        execute_storage(&storage, StorageOperation::ReadHistoryEntries).unwrap(),
        StorageValue::HistoryEntries(vec![])
    );
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![HistoryRecord {
                source: None,
                turn: Some("turn-1".into()),
                kind: "message".into(),
                content: "user".into(),
                payload: json!({"type":"user"}),
                created_at_ms: Some(0),
            }],
        },
    )
    .unwrap();
    let StorageValue::HistoryEntries(history) =
        execute_storage(&storage, StorageOperation::ReadHistoryEntries).unwrap()
    else {
        panic!()
    };
    assert_eq!(history[0].sequence, 1);
    let mut next = record();
    next.created_at_ms = Some(7);
    let second = append_agent_log(&storage, next).unwrap();
    assert_eq!(second.sequence, 2);
    assert_eq!(second.record.created_at_ms, Some(7));
    assert_eq!(
        read_agent_logs(&storage, Some(1), 1).unwrap(),
        vec![second.clone()]
    );
    assert!(read_agent_logs(&storage, None, 0).unwrap().is_empty());
    assert!(read_agent_logs(&storage, Some(u64::MAX), usize::MAX)
        .unwrap()
        .is_empty());
    drop(storage);
    let reopened = open_storage(&db_path, &config, "").unwrap();
    assert_eq!(
        read_agent_logs(&reopened, None, usize::MAX).unwrap(),
        vec![saved, second]
    );
}

#[test]
fn all_levels_and_sources_round_trip_with_absent_optional_fields() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let mut saved = Vec::new();
    for level in [
        AgentLogLevel::Debug,
        AgentLogLevel::Info,
        AgentLogLevel::Warn,
        AgentLogLevel::Error,
    ] {
        for source in [
            AgentLogSource::Driver,
            AgentLogSource::Tool,
            AgentLogSource::Host,
        ] {
            saved.push(
                append_agent_log(
                    &storage,
                    AgentLogRecord {
                        created_at_ms: Some(0),
                        level,
                        source,
                        stage: String::new(),
                        message: String::new(),
                        details_json: None,
                        context_json: None,
                        origin: None,
                        raw_error: None,
                        traceback: None,
                    },
                )
                .unwrap(),
            );
        }
    }
    assert_eq!(read_agent_logs(&storage, None, 100).unwrap(), saved);
    for entry in saved {
        assert_eq!(
            read_agent_logs(&storage, Some(entry.sequence - 1), 1).unwrap(),
            vec![entry]
        );
    }
}

#[test]
fn concurrent_appends_allocate_unique_sequences_on_shared_connection() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let storage = storage.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (0..12)
                    .map(|_| append_agent_log(&storage, record()).unwrap().sequence)
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut sequences: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    sequences.sort_unstable();
    assert_eq!(sequences, (1..=96).collect::<Vec<_>>());
    assert_eq!(read_agent_logs(&storage, None, 200).unwrap().len(), 96);
}

#[test]
fn invalid_input_is_rejected_without_consuming_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    for bad in [json!([]), json!(null), json!("text"), json!(7), json!(true)] {
        for context in [false, true] {
            let mut input = record();
            if context {
                input.context_json = Some(bad.clone());
            } else {
                input.details_json = Some(bad.clone());
            }
            assert_eq!(
                append_agent_log(&storage, input).unwrap_err().kind,
                StorageErrorKind::InvalidRecord
            );
        }
    }
    let mut input = record();
    input.created_at_ms = Some(u64::MAX);
    assert_eq!(
        append_agent_log(&storage, input).unwrap_err().kind,
        StorageErrorKind::InvalidRecord
    );
    assert_eq!(append_agent_log(&storage, record()).unwrap().sequence, 1);
}

#[test]
fn corrupt_persisted_fields_fail_strict_audit() {
    for assignment in [
        "level='fatal'",
        "source='script'",
        "details_json='[]'",
        "context_json='null'",
        "context_json='invalid'",
        "created_at_ms=-1",
        "created_at_ms=1.5",
        "sequence=0",
        "stage=x'ff'",
        "message=x'ff'",
        "origin=x'ff'",
        "raw_error=x'ff'",
        "traceback=x'ff'",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
        append_agent_log(&storage, record()).unwrap();
        let external = Connection::open(&path).unwrap();
        external
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        external
            .execute(&format!("UPDATE agent_logs SET {assignment}"), [])
            .unwrap();
        assert_eq!(
            read_agent_logs(&storage, None, 10).unwrap_err().kind,
            StorageErrorKind::SchemaMismatch,
            "{assignment}"
        );
    }
}

#[test]
fn log_schema_has_exact_columns_and_enforces_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    let db = Connection::open(&path).unwrap();
    let columns: Vec<(String, String, i64, i64)> = db
        .prepare("PRAGMA table_info(agent_logs)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(5)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        vec![
            ("sequence".into(), "INTEGER".into(), 0, 1),
            ("created_at_ms".into(), "INTEGER".into(), 1, 0),
            ("level".into(), "TEXT".into(), 1, 0),
            ("source".into(), "TEXT".into(), 1, 0),
            ("stage".into(), "TEXT".into(), 1, 0),
            ("message".into(), "TEXT".into(), 1, 0),
            ("details_json".into(), "TEXT".into(), 0, 0),
            ("context_json".into(), "TEXT".into(), 0, 0),
            ("origin".into(), "TEXT".into(), 0, 0),
            ("raw_error".into(), "TEXT".into(), 0, 0),
            ("traceback".into(), "TEXT".into(), 0, 0),
        ]
    );
    append_agent_log(&storage, record()).unwrap();
    for assignment in [
        "level='fatal'",
        "source='script'",
        "details_json='[]'",
        "context_json='null'",
        "created_at_ms=-1",
        "created_at_ms=1.5",
        "stage=NULL",
        "message=NULL",
        "raw_error=x'ff'",
    ] {
        assert!(
            db.execute(&format!("UPDATE agent_logs SET {assignment}"), [])
                .is_err(),
            "{assignment}"
        );
    }
    db.execute("UPDATE agent_logs SET sequence=?1", [i64::MAX])
        .unwrap();
    assert_eq!(
        append_agent_log(&storage, record()).unwrap_err().kind,
        StorageErrorKind::InvalidRecord
    );
    assert_eq!(
        read_agent_logs(&storage, None, 1).unwrap()[0].sequence,
        i64::MAX as u64
    );
}
