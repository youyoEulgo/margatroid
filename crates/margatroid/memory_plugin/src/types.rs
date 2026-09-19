use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_plugin::{AgentMemoryStore, AgentMemoryStoreError, HistoryMessage};
use margatroid_types::{Message, TokenUsage, ToolDefinition};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::{MemoryError, MemoryErrorKind};

const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    content TEXT,
    payload TEXT,
    source TEXT
);
CREATE TABLE IF NOT EXISTS state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

pub struct AgentMemory {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl AgentMemory {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MemoryError> {
        let path = path.into();
        validate_path(&path)?;
        let parent = path.parent().ok_or_else(|| {
            MemoryError::new(
                MemoryErrorKind::InvalidPath,
                "memory database path has no parent directory",
            )
        })?;
        fs::create_dir_all(parent).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DirectoryCreateFailed,
                "memory database parent directory could not be created",
            )
        })?;
        let mut connection = Connection::open(&path).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::OpenFailed,
                "memory database could not be opened",
            )
        })?;
        initialize_schema(&mut connection)?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn history_messages(&self) -> Result<Vec<HistoryMessage>, MemoryError> {
        let connection = lock_connection(self)?;
        load_history_messages(&connection)
    }
}

impl AgentMemoryStore for AgentMemory {
    fn append_history(
        &self,
        turn_id: &str,
        source: &str,
        message: &Message,
        tool_schema: &[ToolDefinition],
        usage: Option<&TokenUsage>,
    ) -> Result<(), AgentMemoryStoreError> {
        let mut connection = lock_connection(self).map_err(memory_store_error)?;
        let transaction = connection.transaction().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })?;
        insert_history_message_values(
            &transaction,
            turn_id,
            source,
            message,
            tool_schema,
            usage,
            current_unix_milliseconds().map_err(memory_store_error)?,
        )
        .map_err(memory_store_error)?;
        transaction.commit().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })
    }

    fn set_state(&self, key: &str, value: &str) -> Result<(), AgentMemoryStoreError> {
        let connection = lock_connection(self).map_err(memory_store_error)?;
        let now = current_unix_milliseconds().map_err(memory_store_error)?;
        connection
            .execute(
                "INSERT INTO state (key, value, updated_at_ms) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
                params![key, value, now],
            )
            .map_err(schema_error)
            .map_err(memory_store_error)?;
        Ok(())
    }

    fn state_value(&self, key: &str) -> Result<Option<String>, AgentMemoryStoreError> {
        let connection = lock_connection(self).map_err(memory_store_error)?;
        connection
            .query_row(
                "SELECT value FROM state WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(read_error)
            .map_err(memory_store_error)
    }

    fn append_record(
        &self,
        kind: &str,
        content: &str,
        payload: &str,
        source: &str,
    ) -> Result<(), AgentMemoryStoreError> {
        let mut connection = lock_connection(self).map_err(memory_store_error)?;
        let transaction = connection.transaction().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })?;
        transaction
            .execute(
                "INSERT INTO history_messages (kind, created_at_ms, content, payload, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    kind,
                    current_unix_milliseconds().map_err(memory_store_error)?,
                    content,
                    payload,
                    source
                ],
            )
            .map_err(schema_error)
            .map_err(memory_store_error)?;
        transaction.commit().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })
    }

    fn history_messages(&self) -> Result<Vec<HistoryMessage>, AgentMemoryStoreError> {
        AgentMemory::history_messages(self).map_err(memory_store_error)
    }
}

fn memory_store_error(error: MemoryError) -> AgentMemoryStoreError {
    AgentMemoryStoreError {
        kind: format!("{:?}", error.kind()),
        message: error.message().to_owned(),
    }
}

fn validate_path(path: &Path) -> Result<(), MemoryError> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(MemoryError::new(
            MemoryErrorKind::InvalidPath,
            "memory database path is invalid",
        ));
    }
    Ok(())
}

fn lock_connection<'a>(
    memory: &'a AgentMemory,
) -> Result<std::sync::MutexGuard<'a, Connection>, MemoryError> {
    memory.connection.lock().map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "memory database lock is poisoned",
        )
    })
}

fn initialize_schema(connection: &mut Connection) -> Result<(), MemoryError> {
    let history_is_current = table_has_column(connection, "history_messages", "kind")?;
    let stale_history = table_exists(connection, "history_messages")? && !history_is_current;
    let legacy_state = table_exists(connection, "setting")?;
    let transaction = connection.transaction().map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::SchemaFailed,
            "memory schema transaction failed",
        )
    })?;
    if stale_history {
        transaction
            .execute("DROP TABLE history_messages", [])
            .map_err(schema_error)?;
    }
    for leftover in ["history_messages_legacy", "history_messages_layout_legacy"] {
        transaction
            .execute(&format!("DROP TABLE IF EXISTS {leftover}"), [])
            .map_err(schema_error)?;
    }
    transaction
        .execute_batch(HISTORY_SCHEMA)
        .map_err(schema_error)?;
    if legacy_state {
        transaction
            .execute(
                "INSERT INTO state (key, value, updated_at_ms) SELECT substr(key, 11), value, updated_at_ms FROM setting WHERE key LIKE 'mcl.state/%' ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
                [],
            )
            .map_err(schema_error)?;
        transaction
            .execute("DROP TABLE setting", [])
            .map_err(schema_error)?;
    }
    transaction.commit().map_err(schema_error)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, MemoryError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(schema_error)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, MemoryError> {
    let query = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&query).map_err(schema_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(schema_error)?;
    for current in columns {
        if current.map_err(schema_error)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn schema_error(_: rusqlite::Error) -> MemoryError {
    MemoryError::new(
        MemoryErrorKind::SchemaFailed,
        "memory database schema could not be initialized",
    )
}

fn load_history_messages(connection: &Connection) -> Result<Vec<HistoryMessage>, MemoryError> {
    let mut statement = connection
        .prepare("SELECT sequence, kind, created_at_ms, content, payload, source FROM history_messages ORDER BY sequence ASC")
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(HistoryMessage {
                sequence: row.get(0)?,
                kind: row.get(1)?,
                created_at_ms: row.get(2)?,
                content: row
                    .get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| "{}".to_owned()),
                payload: row
                    .get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| "{}".to_owned()),
                source: row.get(5)?,
            })
        })
        .map_err(read_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(read_error)
}

fn insert_history_message_values(
    transaction: &Transaction<'_>,
    turn_id: &str,
    source: &str,
    message: &Message,
    tool_schema: &[ToolDefinition],
    usage: Option<&TokenUsage>,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    let encode = |value: &serde_json::Value| {
        serde_json::to_string(value).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "history entry could not be encoded",
            )
        })
    };
    let (kind, content, mut payload) = match message {
        Message::User { content } => (
            "message.user",
            serde_json::json!({ "content": content }),
            serde_json::json!({}),
        ),
        Message::Assistant {
            reasoning,
            content,
            tool_calls,
        } => (
            "message.assistant",
            serde_json::json!({ "reasoning": reasoning, "content": content }),
            serde_json::json!({ "tool_calls": tool_calls }),
        ),
        Message::Tool {
            resource_id,
            tool_call_id,
            content,
        } => (
            "message.tool",
            serde_json::json!({ "content": content }),
            serde_json::json!({
                "resource_id": resource_id.to_string(),
                "tool_call_id": tool_call_id,
            }),
        ),
        Message::Error { message } => (
            "message.error",
            serde_json::json!({ "content": message }),
            serde_json::json!({}),
        ),
        Message::System { .. } => {
            return Err(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "system messages cannot be stored as history",
            ))
        }
        Message::Inject { .. } => {
            return Err(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "an inject wrapper is not a history entry",
            ))
        }
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert("turn_id".to_owned(), serde_json::json!(turn_id));
        if matches!(message, Message::Assistant { .. }) {
            object.insert(
                "tool_schema".to_owned(),
                serde_json::to_value(tool_schema).unwrap_or_default(),
            );
            if let Some(usage) = usage {
                object.insert(
                    "input_tokens".to_owned(),
                    serde_json::json!(usage.input_tokens),
                );
                object.insert(
                    "output_tokens".to_owned(),
                    serde_json::json!(usage.output_tokens),
                );
                object.insert(
                    "cache_hit_tokens".to_owned(),
                    serde_json::json!(usage.cache_hit_tokens),
                );
            }
        }
    }
    transaction
        .execute(
            "INSERT INTO history_messages (kind, created_at_ms, content, payload, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, created_at_ms, encode(&content)?, encode(&payload)?, source],
        )
        .map_err(schema_error)?;
    Ok(())
}

fn current_unix_milliseconds() -> Result<i64, MemoryError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "system clock is before Unix epoch",
        )
    })?;
    i64::try_from(duration.as_millis()).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "system timestamp exceeds SQLite integer range",
        )
    })
}

fn read_error(_: rusqlite::Error) -> MemoryError {
    MemoryError::new(MemoryErrorKind::ReadFailed, "memory database read failed")
}
