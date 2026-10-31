use rusqlite::Connection;
use serde_json::json;
use std::fs;
use storage_core::*;

const HISTORY:&str="CREATE TABLE history_records(sequence INTEGER PRIMARY KEY AUTOINCREMENT,key TEXT NOT NULL UNIQUE CHECK(length(key)>0),source TEXT NOT NULL,turn TEXT,kind TEXT NOT NULL,content TEXT NOT NULL,payload TEXT NOT NULL);";
const BLOCKS:&str="CREATE TABLE block_definitions(id INTEGER PRIMARY KEY,agent TEXT NOT NULL,instance TEXT NOT NULL,name TEXT NOT NULL CHECK(length(name)>0),temporary INTEGER NOT NULL CHECK(temporary IN (0,1)),schema_json TEXT NOT NULL,UNIQUE(agent,instance,name),CHECK(temporary=1 OR instance=''));CREATE TABLE block_fields(definition_id INTEGER NOT NULL REFERENCES block_definitions(id),name TEXT NOT NULL CHECK(length(name)>0),inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id,name));";
fn legacy(db: &Connection) {
    db.execute_batch(&format!("{HISTORY}{BLOCKS}")).unwrap();
    db.execute(
        "INSERT INTO block_definitions VALUES(1,'agent:old/name:latest','','conversation',0,?1)",
        [r#"{"messages":"Message","tools":"Tool","sandboxes":"Sandbox"}"#],
    )
    .unwrap();
    for (name, kind, value) in [
        (
            "messages",
            "MESSAGE",
            json!({"Message":[{"role":"assistant","content":"完整消息","metadata":{"nested":[1,null]}}]}),
        ),
        ("tools", "TOOL", json!({"Tool":["tool:local/bash:latest"]})),
        (
            "sandboxes",
            "SANDBOX",
            json!({"Sandbox":["sandbox:local/work:latest"]}),
        ),
    ] {
        db.execute(
            "INSERT INTO block_fields VALUES(1,?1,?2,?3)",
            rusqlite::params![name, kind, value.to_string()],
        )
        .unwrap();
    }
    db.execute("INSERT INTO history_records VALUES(7,'old-key','agent:old/name:latest','old-turn','audit','old content',?1)",[json!({"identity":"agent:old/name:latest"}).to_string()]).unwrap();
}
#[test]
fn known_legacy_migration_preserves_original_typed_resources_namespace_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let cfg = dir.path().join("cfg");
    let db = Connection::open(&path).unwrap();
    legacy(&db);
    let before: Vec<(String, String, String)> = db
        .prepare("SELECT name,inner_type,value_json FROM block_fields ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    {
        let storage = open_storage(&path, &cfg, "").unwrap();
        assert_eq!(
            execute_storage(
                &storage,
                StorageOperation::CreateBlock {
                    name: "conversation".into(),
                    fields: vec!["messages".into()],
                    instance: None
                }
            )
            .unwrap(),
            StorageValue::Unit
        );
        assert_eq!(
            execute_storage(
                &storage,
                StorageOperation::ReadMessages {
                    selectors: vec![MessageSelector {
                        block: "conversation".into(),
                        field: "messages".into(),
                        instance: None,
                        selection: MessageSelection::All
                    }]
                }
            )
            .unwrap(),
            StorageValue::Messages(vec![
                json!({"role":"assistant","content":"完整消息","metadata":{"nested":[1,null]}})
            ])
        );
        let StorageValue::History(history) =
            execute_storage(&storage, StorageOperation::ReadHistory).unwrap()
        else {
            panic!()
        };
        assert_eq!(history[0].created_at_ms, Some(0));
        assert_eq!(history[0].source, "agent:old/name:latest");
        assert_eq!(history[0].turn.as_deref(), Some("old-turn"));
    }
    let after: Vec<(String, String, String)> = db
        .prepare("SELECT name,inner_type,value_json FROM block_fields ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        db.query_row("SELECT agent FROM block_definitions", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "agent:old/name:latest"
    );
    assert_eq!(
        db.query_row("SELECT sequence FROM history_records", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
    open_storage(&path, &cfg, "").unwrap();
}
#[test]
fn ambiguous_namespaces_and_unknown_typed_data_fail_without_schema_or_data_changes() {
    for ambiguous in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");
        let db = Connection::open(&path).unwrap();
        legacy(&db);
        if ambiguous {
            db.execute("INSERT INTO block_definitions VALUES(2,'agent:other/name:latest','','other',0,'{}')",[]).unwrap();
        } else {
            db.execute(
                "UPDATE block_fields SET inner_type='UNKNOWN' WHERE name='tools'",
                [],
            )
            .unwrap();
        }
        drop(db);
        let before = fs::read(&path).unwrap();
        assert_eq!(
            open_storage(&path, &dir.path().join("cfg"), "")
                .err()
                .unwrap()
                .kind,
            StorageErrorKind::SchemaMismatch
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!dir.path().join("cfg").exists());
    }
}
#[test]
fn unknown_schema_is_refused_without_creating_tables_or_rewriting_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE block_definitions(id INTEGER PRIMARY KEY, schema_json TEXT);INSERT INTO block_definitions VALUES(1,'keep');").unwrap();
    drop(db);
    let before = fs::read(&path).unwrap();
    assert_eq!(
        open_storage(&path, &dir.path().join("cfg"), "")
            .err()
            .unwrap()
            .kind,
        StorageErrorKind::SchemaMismatch
    );
    assert_eq!(fs::read(path).unwrap(), before);
}
