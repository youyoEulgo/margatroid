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
    for (table, identity_columns, insert_columns, identity_values) in [
        (
            "agent_messages",
            [("turn", 0, 0), ("type", 1, 0)],
            "turn,type",
            "NULL,'user'",
        ),
        (
            "mcl_records",
            [("source", 1, 0), ("turn", 0, 0)],
            "source,turn",
            "'source',NULL",
        ),
    ] {
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
                (
                    identity_columns[0].0.into(),
                    identity_columns[0].1,
                    identity_columns[0].2
                ),
                (
                    identity_columns[1].0.into(),
                    identity_columns[1].1,
                    identity_columns[1].2
                ),
                ("content".into(), 1, 0),
                ("payload".into(), 1, 0),
                ("created_at_ms".into(), 1, 0),
            ]
        );
        for body in [
            "NULL,'{}',0",
            "'body',NULL,0",
            "'body','{}',NULL",
            "'body','{}',-1",
        ] {
            assert!(db.execute(&format!("INSERT INTO {table}({insert_columns},content,payload,created_at_ms) VALUES({identity_values},{body})"), []).is_err());
        }
        let identity = if table == "agent_messages" {
            "type"
        } else {
            "source"
        };
        for value in ["NULL", "''"] {
            assert!(db.execute(&format!("INSERT INTO {table}({identity},content,payload,created_at_ms) VALUES({value},'body','{{}}',0)"), []).is_err());
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
