use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_plugin::{AgentMemoryStore, AgentMemoryStoreError, HistoryMessage};
use margatroid_types::{MclMessage, Message, TokenUsage, ToolDefinition};
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
CREATE TABLE IF NOT EXISTS realtime_context (
    position INTEGER PRIMARY KEY,
    message TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_hit_tokens INTEGER
);
CREATE TABLE IF NOT EXISTS setting (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

pub struct AgentMemory {
    path: PathBuf,
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealtimeContext {
    pub messages: Vec<Message>,
    pub tool_context: Vec<Message>,
    pub ordered_messages: Vec<Message>,
    pub token_usage: TokenUsage,
    pub last_input_tokens: u64,
}


impl AgentMemory {
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, RealtimeContext), MemoryError> {
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
        let mut context = load_realtime_context(&connection)?;
        context.token_usage = load_token_usage(&connection)?;
        context.last_input_tokens = load_last_input_tokens(&connection)?;
        Ok((
            Self {
                path,
                connection: Mutex::new(connection),
            },
            context,
        ))
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

    fn setting_value(&self, key: &str) -> Result<Vec<String>, AgentMemoryStoreError> {
        let connection = lock_connection(self).map_err(memory_store_error)?;
        let value = connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(read_error)
            .map_err(memory_store_error)?;
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                error.to_string(),
            ))
        })
    }

    fn set_setting(&self, entries: &[(String, String)]) -> Result<(), AgentMemoryStoreError> {
        let mut connection = lock_connection(self).map_err(memory_store_error)?;
        let transaction = connection.transaction().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })?;
        let now = current_unix_milliseconds().map_err(memory_store_error)?;
        for (key, value) in entries {
            transaction
                .execute(
                    "INSERT INTO setting (key, value, updated_at_ms) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
                    params![key, value, now],
                )
                .map_err(schema_error)
                .map_err(memory_store_error)?;
        }
        transaction.commit().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })
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

    fn rewrite_realtime(&self, messages: &[MclMessage]) -> Result<(), AgentMemoryStoreError> {
        let mut connection = lock_connection(self).map_err(memory_store_error)?;
        let transaction = connection.transaction().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })?;
        let entries = messages
            .iter()
            .cloned()
            .map(|entry| MclMessage {
                message: entry.message,
                usage: entry.usage,
            })
            .collect::<Vec<_>>();
        rewrite_realtime_context(&transaction, &entries).map_err(memory_store_error)?;
        transaction.commit().map_err(|error| {
            memory_store_error(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                error.to_string(),
            ))
        })
    }

    fn read_realtime(&self) -> Result<Vec<MclMessage>, AgentMemoryStoreError> {
        let connection = lock_connection(self).map_err(memory_store_error)?;
        load_ordered_realtime_messages(&connection)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| MclMessage {
                        message: entry.message,
                        usage: entry.usage,
                    })
                    .collect()
            })
            .map_err(memory_store_error)
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
    let legacy_realtime = table_has_column(connection, "realtime_messages", "position")?
        && !table_has_column(connection, "realtime_messages", "context")?;
    let realtime_context_exists = table_exists(connection, "realtime_context")?;
    let realtime_has_input = table_has_column(connection, "realtime_context", "input_tokens")?;
    let realtime_has_output = table_has_column(connection, "realtime_context", "output_tokens")?;
    let realtime_has_cache = table_has_column(connection, "realtime_context", "cache_hit_tokens")?;
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
    if legacy_realtime {
        transaction
            .execute(
                "ALTER TABLE realtime_messages RENAME TO realtime_messages_legacy",
                [],
            )
            .map_err(schema_error)?;
    }
    transaction
        .execute_batch(HISTORY_SCHEMA)
        .map_err(schema_error)?;
    if realtime_context_exists {
        for (missing, column) in [
            (!realtime_has_input, "input_tokens"),
            (!realtime_has_output, "output_tokens"),
            (!realtime_has_cache, "cache_hit_tokens"),
        ] {
            if missing {
                transaction
                    .execute(
                        &format!("ALTER TABLE realtime_context ADD COLUMN {column} INTEGER"),
                        [],
                    )
                    .map_err(schema_error)?;
            }
        }
    }
    if legacy_realtime {
        migrate_realtime(&transaction)?;
        transaction
            .execute("DROP TABLE realtime_messages_legacy", [])
            .map_err(schema_error)?;
    }
    transaction
        .execute("DROP TABLE IF EXISTS realtime_messages", [])
        .map_err(schema_error)?;
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



fn migrate_realtime(transaction: &Transaction<'_>) -> Result<(), MemoryError> {
    let mut statement = transaction
        .prepare("SELECT message FROM realtime_messages_legacy ORDER BY position")
        .map_err(schema_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(schema_error)?;
    let mut context = RealtimeContext::default();
    for row in rows {
        let encoded = row.map_err(schema_error)?;
        let message = serde_json::from_str(&encoded).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "legacy realtime message could not be decoded",
            )
        })?;
        match message {
            Message::User { .. } | Message::Assistant { .. } => {
                context.messages.push(message.clone());
                context.ordered_messages.push(message);
            }
            Message::Tool { .. } => {
                context.tool_context.push(message.clone());
                context.ordered_messages.push(message);
            }
            Message::System { .. } | Message::Error { .. } => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "legacy realtime context contains an invalid message",
                ));
            }
        }
    }
    rewrite_realtime_context(
        transaction,
        &context
            .ordered_messages
            .into_iter()
            .map(|message| MclMessage {
                message,
                usage: None,
            })
            .collect::<Vec<_>>(),
    )
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
                content: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "{}".to_owned()),
                payload: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "{}".to_owned()),
                source: row.get(5)?,
            })
        })
        .map_err(read_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(read_error)
}

fn load_token_usage(connection: &Connection) -> Result<TokenUsage, MemoryError> {
    let mut usage = TokenUsage::default();
    for entry in load_history_messages(connection)? {
        if let Some(entry_usage) = entry.usage() {
            usage.input_tokens += entry_usage.input_tokens;
            usage.output_tokens += entry_usage.output_tokens;
            usage.cache_hit_tokens += entry_usage.cache_hit_tokens;
        }
    }
    Ok(usage)
}

fn load_last_input_tokens(connection: &Connection) -> Result<u64, MemoryError> {
    Ok(load_history_messages(connection)?
        .iter()
        .rev()
        .find_map(|entry| entry.usage().map(|usage| usage.input_tokens))
        .unwrap_or_default())
}


fn load_realtime_context(connection: &Connection) -> Result<RealtimeContext, MemoryError> {
    let entries = load_ordered_realtime_messages(connection)?;
    let ordered_messages = entries
        .iter()
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>();
    Ok(RealtimeContext {
        messages: ordered_messages
            .iter()
            .filter(|message| matches!(message, Message::User { .. } | Message::Assistant { .. }))
            .cloned()
            .collect(),
        tool_context: ordered_messages
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .cloned()
            .collect(),
        ordered_messages,
        ..RealtimeContext::default()
    })
}

fn load_ordered_realtime_messages(connection: &Connection) -> Result<Vec<MclMessage>, MemoryError> {
    let mut statement = connection
        .prepare("SELECT position, message, input_tokens, output_tokens, cache_hit_tokens FROM realtime_context ORDER BY position")
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(read_error)?;
    let mut messages = Vec::new();
    for row in rows {
        let (position, encoded, input, output, cache_hit) = row.map_err(read_error)?;
        if position != messages.len() as i64 {
            return Err(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "ordered realtime message positions are not continuous",
            ));
        }
        let message = serde_json::from_str::<Message>(&encoded).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "ordered realtime message JSON could not be decoded",
            )
        })?;
        let usage = match (input, output, cache_hit) {
            (None, None, None) => None,
            (Some(input), Some(output), Some(cache_hit)) => Some(TokenUsage {
                input_tokens: u64::try_from(input).map_err(|_| {
                    MemoryError::new(
                        MemoryErrorKind::DecodeFailed,
                        "realtime input token usage is negative",
                    )
                })?,
                output_tokens: u64::try_from(output).map_err(|_| {
                    MemoryError::new(
                        MemoryErrorKind::DecodeFailed,
                        "realtime output token usage is negative",
                    )
                })?,
                cache_hit_tokens: u64::try_from(cache_hit).map_err(|_| {
                    MemoryError::new(
                        MemoryErrorKind::DecodeFailed,
                        "realtime cache token usage is negative",
                    )
                })?,
            }),
            _ => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "realtime token usage is incomplete",
                ))
            }
        };
        messages.push(MclMessage { message, usage });
    }
    Ok(messages)
}

fn rewrite_realtime_context(
    transaction: &Transaction<'_>,
    ordered_messages: &[MclMessage],
) -> Result<(), MemoryError> {
    transaction
        .execute("DELETE FROM realtime_context", [])
        .map_err(write_error)?;
    for (position, entry) in ordered_messages.iter().enumerate() {
        let encoded = serde_json::to_string(&entry.message).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "ordered realtime message JSON could not be encoded",
            )
        })?;
        transaction
            .execute(
                "INSERT INTO realtime_context (position, message, input_tokens, output_tokens, cache_hit_tokens) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    position as i64,
                    encoded,
                    entry.usage.as_ref().map(|usage| usage.input_tokens as i64),
                    entry.usage.as_ref().map(|usage| usage.output_tokens as i64),
                    entry.usage.as_ref().map(|usage| usage.cache_hit_tokens as i64),
                ],
            )
            .map_err(write_error)?;
    }
    Ok(())
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
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert("turn_id".to_owned(), serde_json::json!(turn_id));
        if matches!(message, Message::Assistant { .. }) {
            object.insert(
                "tool_schema".to_owned(),
                serde_json::to_value(tool_schema).unwrap_or_default(),
            );
            if let Some(usage) = usage {
                object.insert("input_tokens".to_owned(), serde_json::json!(usage.input_tokens));
                object.insert("output_tokens".to_owned(), serde_json::json!(usage.output_tokens));
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

fn write_error(_: rusqlite::Error) -> MemoryError {
    MemoryError::new(MemoryErrorKind::WriteFailed, "memory database write failed")
}
