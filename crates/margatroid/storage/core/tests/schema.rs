use rusqlite::Connection;
use storage_core::*;

#[test]
fn new_database_contains_only_current_four_tables_and_reopens() {
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
    assert_eq!(
        tables,
        [
            "agent_logs",
            "agent_messages",
            "block_snapshots",
            "mcl_records"
        ]
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(agent_messages)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "sequence",
            "turn",
            "role",
            "content",
            "data",
            "tool_call",
            "payload",
            "created_at_ms"
        ]
    );
    for values in [
        "1,'user',NULL,'{}',0",
        "1,'user','text',NULL,0",
        "1,'','text','{}',0",
        "1,NULL,'text','{}',0",
        "1,'user','text','{}',-1",
    ] {
        assert!(db.execute(&format!("INSERT INTO agent_messages(sequence,role,content,payload,created_at_ms) VALUES({values})"),[]).is_err());
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
fn old_three_table_database_is_rejected_without_alter_or_backup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE agent_messages(sequence INTEGER PRIMARY KEY,turn TEXT,type TEXT NOT NULL CHECK(length(type)>0),content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));CREATE TABLE mcl_records(sequence INTEGER PRIMARY KEY,source TEXT NOT NULL CHECK(length(source)>0),turn TEXT,content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));CREATE TABLE blocks(name TEXT PRIMARY KEY NOT NULL CHECK(length(name)>0),fields_json TEXT NOT NULL CHECK(json_valid(fields_json) AND json_type(fields_json)='object'));INSERT INTO blocks VALUES('keep','{}');PRAGMA user_version=2;").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let error = open_storage(&path, &directory.path().join("settings.toml"), "")
        .err()
        .unwrap();
    assert_eq!(error.kind, StorageErrorKind::SchemaMismatch);
    assert!(error.message.contains("four-table"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn changed_agent_log_columns_or_constraints_are_rejected_without_rewriting() {
    for mutation in [
        "ALTER TABLE agent_logs ADD COLUMN unexpected TEXT",
        "DROP TABLE agent_logs;CREATE TABLE agent_logs(sequence INTEGER PRIMARY KEY,created_at_ms INTEGER NOT NULL,level TEXT NOT NULL,source TEXT NOT NULL,stage TEXT NOT NULL,message TEXT NOT NULL,details_json TEXT,context_json TEXT,origin TEXT,raw_error TEXT,traceback TEXT)",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite");
        let settings = directory.path().join("settings.toml");
        drop(open_storage(&path, &settings, "").unwrap());
        let db = Connection::open(&path).unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        assert_eq!(open_storage(&path, &settings, "").err().unwrap().kind, StorageErrorKind::SchemaMismatch);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }
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

#[test]
fn version_three_json_block_database_is_rejected_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let settings = directory.path().join("settings.toml");
    drop(open_storage(&path, &settings, "").unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE block_snapshots;CREATE TABLE blocks(name TEXT PRIMARY KEY NOT NULL,fields_json TEXT NOT NULL);INSERT INTO blocks VALUES('session','{\"conversation\":[]}');PRAGMA user_version=3;").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        open_storage(&path, &settings, "").err().unwrap().kind,
        StorageErrorKind::SchemaMismatch
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!directory.path().join("state.sqlite.blocks").exists());
}
