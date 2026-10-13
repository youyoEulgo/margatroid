use std::path::Path;
use std::sync::{Arc, Mutex};

use margatroid_types::history_components::{AgentHistoryStore, HistoryError, HistoryRecord};
use rusqlite::Connection;

use crate::history::SqliteHistoryStore;

pub(crate) const BLOCK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS block_definitions (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    instance TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(name) > 0),
    temporary INTEGER NOT NULL CHECK(temporary IN (0, 1)),
    schema_json TEXT NOT NULL,
    UNIQUE(agent, instance, name),
    CHECK(temporary = 1 OR instance = '')
);
CREATE TABLE IF NOT EXISTS block_fields (
    definition_id INTEGER NOT NULL REFERENCES block_definitions(id),
    name TEXT NOT NULL CHECK(length(name) > 0),
    inner_type TEXT NOT NULL,
    value_json TEXT NOT NULL,
    PRIMARY KEY(definition_id, name)
);
"#;

#[derive(Clone)]
pub struct SqliteAgentDatabase {
    pub(crate) connection: Arc<Mutex<Connection>>,
    pub(crate) retired: Arc<std::sync::atomic::AtomicBool>,
}

impl SqliteAgentDatabase {
    pub(crate) fn history_view(&self) -> SqliteHistoryStore {
        SqliteHistoryStore {
            connection: self.connection.clone(),
            retired: self.retired.clone(),
        }
    }

    pub fn open(
        path: impl AsRef<Path>,
    ) -> Result<Self, margatroid_types::block_storage::BlockStorageError> {
        use margatroid_types::block_storage::BlockStorageError;
        let history = SqliteHistoryStore::open(path)
            .map_err(|error| BlockStorageError::Io(error.to_string()))?;
        {
            let mut connection = history
                .connection
                .lock()
                .map_err(|_| BlockStorageError::Io("database connection mutex poisoned".into()))?;
            connection
                .execute_batch("PRAGMA foreign_keys=ON;")
                .map_err(|error| BlockStorageError::Io(error.to_string()))?;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|error| BlockStorageError::Io(error.to_string()))?;
            transaction
                .execute_batch(BLOCK_SCHEMA)
                .map_err(|error| BlockStorageError::Io(error.to_string()))?;
            for query in [
                "SELECT sequence,key,source,turn,kind,content,payload FROM history_records LIMIT 0",
                "SELECT id,agent,instance,name,temporary,schema_json FROM block_definitions LIMIT 0",
                "SELECT definition_id,name,inner_type,value_json FROM block_fields LIMIT 0",
            ] {
                transaction.prepare(query).map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
            }
            for (table, expected) in [
                (
                    "block_definitions",
                    vec![
                        ("id", "INTEGER", false, 1),
                        ("agent", "TEXT", true, 0),
                        ("instance", "TEXT", true, 0),
                        ("name", "TEXT", true, 0),
                        ("temporary", "INTEGER", true, 0),
                        ("schema_json", "TEXT", true, 0),
                    ],
                ),
                (
                    "block_fields",
                    vec![
                        ("definition_id", "INTEGER", true, 1),
                        ("name", "TEXT", true, 2),
                        ("inner_type", "TEXT", true, 0),
                        ("value_json", "TEXT", true, 0),
                    ],
                ),
            ] {
                let columns = transaction
                    .prepare(
                        "SELECT name,type,\"notnull\",pk FROM pragma_table_info(?1) ORDER BY cid",
                    )
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                    .query_map([table], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, bool>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    })
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
                if columns.len() != expected.len()
                    || columns.iter().zip(expected).any(|(actual, required)| {
                        actual.0 != required.0
                            || !actual.1.eq_ignore_ascii_case(required.1)
                            || (required.2 && !actual.2)
                            || actual.3 != required.3
                    })
                {
                    return Err(BlockStorageError::SchemaMismatch(format!(
                        "{table} has incompatible columns or primary key"
                    )));
                }
            }
            let foreign_keys = transaction
                .prepare(
                    "SELECT \"table\",\"from\",\"to\" FROM pragma_foreign_key_list('block_fields')",
                )
                .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
            if !foreign_keys.iter().any(|(table, from, to)| {
                table == "block_definitions" && from == "definition_id" && to == "id"
            }) {
                return Err(BlockStorageError::SchemaMismatch(
                    "block_fields lacks definition foreign key".into(),
                ));
            }
            for (table, expected) in [
                ("block_definitions", vec!["agent", "instance", "name"]),
                ("block_fields", vec!["definition_id", "name"]),
            ] {
                let mut indexes = transaction
                    .prepare(
                        "SELECT name FROM pragma_index_list(?1) WHERE \"unique\"=1 AND partial=0",
                    )
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
                let names = indexes
                    .query_map([table], |row| row.get::<_, String>(0))
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
                let mut found = false;
                for name in names {
                    let columns = transaction
                        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                        .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                        .query_map([name], |row| row.get::<_, String>(0))
                        .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|error| BlockStorageError::SchemaMismatch(error.to_string()))?;
                    if columns == expected {
                        found = true;
                    }
                }
                if !found {
                    return Err(BlockStorageError::SchemaMismatch(format!(
                        "{table} lacks required unique key"
                    )));
                }
            }
            transaction
                .commit()
                .map_err(|error| BlockStorageError::Io(error.to_string()))?;
        }
        Ok(Self {
            connection: history.connection,
            retired: history.retired,
        })
    }
}

impl AgentHistoryStore for SqliteAgentDatabase {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        if self.retired.load(std::sync::atomic::Ordering::Acquire) {
            return Err(HistoryError::CleanupFailed(
                "database connection retired after uncertain transaction".into(),
            ));
        }
        self.history_view().append_batch(records)
    }

    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        if self.retired.load(std::sync::atomic::Ordering::Acquire) {
            return Err(HistoryError::CleanupFailed(
                "database connection retired after uncertain transaction".into(),
            ));
        }
        self.history_view().list()
    }
}
