use core_plugin::App;
use memory_plugin::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord, MemoryPlugin,
    MemoryPluginInstalled, SqliteHistoryStore,
};
use serde_json::json;
use tempfile::tempdir;

fn record(key: &str) -> HistoryRecord {
    HistoryRecord {
        key: key.into(),
        source: "driver".into(),
        turn: Some("turn-1".into()),
        kind: "message.user".into(),
        content: "hello".into(),
        payload: json!({"message": {"role": "user", "content": "hello"}}),
    }
}

#[test]
fn documented_public_api_composes_without_an_agent_or_runtime() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut app = App::new();
    app.add_plugin(MemoryPlugin::default());
    assert!(app.world().contains_resource::<MemoryPluginInstalled>());

    let store = SqliteHistoryStore::open(&path).unwrap();
    let handle = AgentHistoryHandle::new(store.clone());
    let first = record("first");
    handle.append(first.clone()).unwrap();
    assert_eq!(store.list().unwrap(), vec![first.clone()]);
    drop(handle);
    drop(store);
    assert_eq!(
        SqliteHistoryStore::open(&path).unwrap().list().unwrap(),
        vec![first]
    );
}

#[test]
fn external_trait_handle_observes_idempotence_and_atomic_conflicts() {
    let handle = AgentHistoryHandle::new(SqliteHistoryStore::open(":memory:").unwrap());
    let first = record("first");
    handle
        .append_batch(vec![first.clone(), first.clone()])
        .unwrap();
    let mut conflict = first.clone();
    conflict.turn = Some("turn-2".into());
    assert_eq!(
        handle.append_batch(vec![record("second"), conflict]),
        Err(HistoryError::KeyConflict("first".into()))
    );
    assert_eq!(handle.list().unwrap(), vec![first]);
}

#[test]
fn newly_opened_database_contains_only_history() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    SqliteHistoryStore::open(&path).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(tables, ["history_records"]);
}
