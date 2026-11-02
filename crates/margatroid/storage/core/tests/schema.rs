use rusqlite::Connection;
use storage_core::*;

#[test]
fn new_database_contains_only_current_three_tables_and_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    drop(open_storage(&path, &directory.path().join("settings.toml"), "").unwrap());
    let db = Connection::open(&path).unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tables, ["agent_messages", "blocks", "mcl_records"]);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    for table in ["agent_messages", "mcl_records"] {
        let columns: Vec<(String, i64, i64)> = db
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| Ok((r.get(1)?, r.get(3)?, r.get(5)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            columns,
            vec![
                ("sequence".into(), 0, 1),
                ("source".into(), 1, 0),
                ("turn".into(), 0, 0),
                ("content".into(), 1, 0),
                ("payload".into(), 1, 0),
                ("created_at_ms".into(), 1, 0),
            ]
        );
        for values in [
            "NULL,NULL,'body','{}',0",
            "'',NULL,'body','{}',0",
            "'source',NULL,NULL,'{}',0",
            "'source',NULL,'body',NULL,0",
            "'source',NULL,'body','{}',NULL",
            "'source',NULL,'body','{}',-1",
        ] {
            assert!(db.execute(&format!("INSERT INTO {table}(source,turn,content,payload,created_at_ms) VALUES({values})"), []).is_err());
        }
    }
    drop(db);
    drop(open_storage(&path, &directory.path().join("settings.toml"), "").unwrap());
}

#[test]
fn incompatible_database_is_rejected_without_migration_or_backup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE unrelated(value TEXT);INSERT INTO unrelated VALUES('keep');")
        .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        open_storage(&path, &directory.path().join("settings.toml"), "")
            .err()
            .unwrap()
            .kind,
        StorageErrorKind::SchemaMismatch
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn unsupported_schema_version_is_rejected_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    drop(open_storage(&path, &directory.path().join("settings.toml"), "").unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=1;").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        open_storage(&path, &directory.path().join("settings.toml"), "")
            .err()
            .unwrap()
            .kind,
        StorageErrorKind::SchemaMismatch
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
