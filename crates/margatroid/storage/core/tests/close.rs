use serde_json::json;
use storage_core::*;

fn page() -> RecordRange {
    RecordRange {
        before_sequence: None,
        after_sequence: None,
        limit: 10,
    }
}

#[test]
fn explicit_close_retires_all_clones_without_publishing_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("state.sqlite");
    let settings = dir.path().join("setting.toml");
    let storage = open_storage(&database, &settings, "answer = 42\n").unwrap();
    let clone = storage.clone();
    let reopened_clone = open_storage(&database, &settings, "ignored = true\n").unwrap();
    assert!(storage.same_database(&reopened_clone));
    assert!(storage.is_open());
    assert_eq!(storage.settings_path(), settings);
    execute_storage(
        &storage,
        StorageOperation::CreateBlock {
            name: "memory".into(),
            fields: vec!["messages".into()],
            instance: None,
        },
    )
    .unwrap();
    append_message_history(&storage, &[json!({"type":"user","content":"kept"})], None).unwrap();
    let changes = subscribe_record_changes(&storage);
    let watermark = *changes.borrow();
    close_storage(&storage).unwrap();
    for closed in [&storage, &clone, &reopened_clone] {
        assert!(!closed.is_open());
        let operations = [
            StorageOperation::ReadConfig,
            StorageOperation::SetConfig {
                path: "answer".into(),
                value: toml::Value::Integer(0),
            },
            StorageOperation::DeleteConfig {
                path: "answer".into(),
            },
            StorageOperation::ReadHistory,
            StorageOperation::CreateBlock {
                name: "after-close".into(),
                fields: vec!["messages".into()],
                instance: Some("vm".into()),
            },
            StorageOperation::DropTemporaryBlocks {
                instance: "vm".into(),
            },
        ];
        for operation in operations {
            assert_eq!(
                execute_storage(closed, operation).unwrap_err().kind,
                StorageErrorKind::Closed
            );
        }
        assert_eq!(
            append_message_history(closed, &[], None).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_message_page(closed, page()).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_mcl_page(closed, page()).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_agent_log_page(closed, page()).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_record_watermarks(closed).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_agent_logs(closed, None, 0).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            snapshot_block(closed, "memory", None).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            restore_block(closed, "memory", None, 1).unwrap_err().kind,
            StorageErrorKind::Closed
        );
        assert_eq!(
            read_block_snapshots(closed, "memory", None)
                .unwrap_err()
                .kind,
            StorageErrorKind::Closed
        );
    }
    assert_eq!(*changes.borrow(), watermark);
    assert!(!changes.has_changed().unwrap());
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "answer = 42\n");
    close_storage(&clone).unwrap();

    let new = open_storage(&database, &settings, "").unwrap();
    assert!(new.is_open());
    assert!(!new.same_database(&storage));
    assert!(!clone.is_open());
    assert_eq!(read_message_page(&new, page()).unwrap().records.len(), 1);
    close_storage(&new).unwrap();
}
