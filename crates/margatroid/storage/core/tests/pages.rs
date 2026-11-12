use serde_json::json;
use storage_core::*;

fn query(before: Option<u64>, after: Option<u64>, limit: usize) -> RecordRange {
    RecordRange {
        before_sequence: before,
        after_sequence: after,
        limit,
    }
}
fn append_mcl(storage: &AgentStorage) {
    execute_storage(
        storage,
        StorageOperation::AppendHistory {
            records: vec![HistoryRecord {
                source: Some("client:local/test:latest".into()),
                turn: None,
                kind: "mcl".into(),
                content: "GET x".into(),
                payload: json!({"text":"GET x"}),
                created_at_ms: Some(0),
            }],
        },
    )
    .unwrap();
}
#[test]
fn ranges_stay_stable_after_appends_and_keep_records_separate() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(
        &dir.path().join("state.sqlite"),
        &dir.path().join("setting.toml"),
        "",
    )
    .unwrap();
    for n in 0..5 {
        append_message_history(
            &storage,
            &[json!({"type":"user","content":n.to_string()})],
            None,
        )
        .unwrap();
        append_mcl(&storage);
    }
    let latest = read_message_page(&storage, query(None, None, 2)).unwrap();
    assert_eq!(
        latest
            .records
            .iter()
            .map(|r| r.sequence)
            .collect::<Vec<_>>(),
        [7, 9]
    );
    assert_eq!(latest.next_before_sequence, Some(7));
    assert_eq!(latest.next_after_sequence, None);
    append_message_history(
        &storage,
        &[json!({"type":"assistant","reasoning":"thought","content":"new"})],
        None,
    )
    .unwrap();
    let older = read_message_page(&storage, query(Some(7), None, 2)).unwrap();
    assert_eq!(
        older.records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [3, 5]
    );
    assert_eq!(older.next_before_sequence, Some(3));
    assert_eq!(older.next_after_sequence, None);
    let range = read_message_page(&storage, query(Some(10), Some(1), 2)).unwrap();
    assert_eq!(
        range.records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [3, 5]
    );
    assert_eq!(range.next_after_sequence, Some(5));
    assert_eq!(range.next_before_sequence, None);
    for row in &range.records {
        assert_eq!(row.record.payload["sequence"], row.sequence);
        assert_eq!(row.record.kind, "message");
    }
    let audit = read_mcl_page(&storage, query(None, None, 2)).unwrap();
    assert_eq!(
        audit.records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [8, 10]
    );
    assert!(audit.records.iter().all(|r| r.record.kind == "mcl"));
    assert_eq!(
        read_record_watermarks(&storage).unwrap(),
        RecordWatermarks {
            message: 11,
            mcl: 10,
            agent_log: 0
        }
    );
    for n in 0..3 {
        append_agent_log(
            &storage,
            AgentLogRecord {
                created_at_ms: Some(0),
                level: AgentLogLevel::Info,
                source: AgentLogSource::Driver,
                stage: "driver".into(),
                message: n.to_string(),
                details_json: None,
                context_json: None,
                origin: None,
                raw_error: None,
                traceback: None,
            },
        )
        .unwrap();
    }
    let logs = read_agent_log_page(&storage, query(None, None, 2)).unwrap();
    assert_eq!(
        logs.records.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(logs.next_before_sequence, Some(2));
    assert_eq!(
        read_record_watermarks(&storage).unwrap(),
        RecordWatermarks {
            message: 11,
            mcl: 10,
            agent_log: 3
        }
    );
    assert_eq!(
        read_agent_log_page(&storage, query(None, Some(1), 1))
            .unwrap()
            .next_after_sequence,
        Some(2)
    );
    assert_eq!(
        read_agent_log_page(&storage, query(None, None, 2)).unwrap(),
        logs
    );
}

#[test]
fn ranges_validate_boundaries_and_empty_pages() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(
        &dir.path().join("state.sqlite"),
        &dir.path().join("setting.toml"),
        "",
    )
    .unwrap();
    for range in [
        query(None, None, 0),
        query(Some(3), Some(3), 1),
        query(Some(2), Some(3), 1),
        query(None, Some(u64::MAX), 1),
    ] {
        assert_eq!(
            read_message_page(&storage, range).unwrap_err().kind,
            StorageErrorKind::InvalidRange
        );
        assert_eq!(
            read_mcl_page(&storage, range).unwrap_err().kind,
            StorageErrorKind::InvalidRange
        );
        assert_eq!(
            read_agent_log_page(&storage, range).unwrap_err().kind,
            StorageErrorKind::InvalidRange
        );
    }
    let page = read_message_page(&storage, query(Some(1), Some(0), 10)).unwrap();
    assert!(page.records.is_empty());
    assert_eq!(page.next_before_sequence, None);
    assert_eq!(page.next_after_sequence, None);
}

#[test]
fn record_change_watch_notifies_only_after_new_records_commit() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(
        &dir.path().join("state.sqlite"),
        &dir.path().join("setting.toml"),
        "",
    )
    .unwrap();
    let mut changes = subscribe_record_changes(&storage);
    assert!(!changes.has_changed().unwrap());
    let messages = append_message_history(
        &storage,
        &[json!({"type":"user","content":"committed"})],
        None,
    )
    .unwrap();
    assert!(changes.has_changed().unwrap());
    assert_eq!(changes.borrow_and_update().message, 1);
    append_message_history(&storage, &messages, None).unwrap();
    read_message_page(&storage, query(None, None, 50)).unwrap();
    read_record_watermarks(&storage).unwrap();
    execute_storage(
        &storage,
        StorageOperation::AppendHistory { records: vec![] },
    )
    .unwrap();
    assert!(!changes.has_changed().unwrap());
    let failure = execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![
                HistoryRecord {
                    source: Some("client:local/test".into()),
                    turn: None,
                    kind: "mcl".into(),
                    content: "rolled-back".into(),
                    payload: json!({}),
                    created_at_ms: Some(0),
                },
                HistoryRecord {
                    source: None,
                    turn: None,
                    kind: "invalid".into(),
                    content: String::new(),
                    payload: json!({}),
                    created_at_ms: Some(0),
                },
            ],
        },
    );
    assert!(failure.is_err());
    assert!(!changes.has_changed().unwrap());
    assert_eq!(read_record_watermarks(&storage).unwrap().mcl, 0);
    append_mcl(&storage);
    assert_eq!(changes.borrow_and_update().mcl, 2);
    append_agent_log(
        &storage,
        AgentLogRecord {
            created_at_ms: Some(0),
            level: AgentLogLevel::Info,
            source: AgentLogSource::Driver,
            stage: "test".into(),
            message: "new log".into(),
            details_json: None,
            context_json: None,
            origin: None,
            raw_error: None,
            traceback: None,
        },
    )
    .unwrap();
    assert_eq!(changes.borrow_and_update().agent_log, 1);
    let reopened = open_storage(
        &dir.path().join("state.sqlite"),
        &dir.path().join("setting.toml"),
        "",
    )
    .unwrap();
    assert!(storage.same_database(&reopened));
    append_mcl(&reopened);
    assert_eq!(changes.borrow_and_update().mcl, 3);
}
