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
    turn_id TEXT NOT NULL,
    role TEXT NOT NULL,
    reasoning TEXT,
    content TEXT,
    tool_calls TEXT NOT NULL,
    tool_schema TEXT NOT NULL,
    resource_id TEXT,
    tool_call_id TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS realtime_context (
    position INTEGER PRIMARY KEY,
    message TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_hit_tokens INTEGER
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

#[derive(Clone, Copy)]
struct HistoryLayout {
    has_reasoning: bool,
    has_resource_id: bool,
    has_tool_call_id: bool,
    has_tool_schema: bool,
    has_input_tokens: bool,
    has_output_tokens: bool,
    has_cache_hit_tokens: bool,
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
    let legacy_history = table_has_column(connection, "history_messages", "message")?;
    let history_exists = table_exists(connection, "history_messages")?;
    let history_has_resource_id = table_has_column(connection, "history_messages", "resource_id")?;
    let history_has_tool_call_id =
        table_has_column(connection, "history_messages", "tool_call_id")?;
    let history_has_reasoning = table_has_column(connection, "history_messages", "reasoning")?;
    let history_has_tool_schema = table_has_column(connection, "history_messages", "tool_schema")?;
    let history_has_input_tokens =
        table_has_column(connection, "history_messages", "input_tokens")?;
    let history_has_output_tokens =
        table_has_column(connection, "history_messages", "output_tokens")?;
    let history_has_cache_hit_tokens =
        table_has_column(connection, "history_messages", "cache_hit_tokens")?;
    let history_layout = HistoryLayout {
        has_reasoning: history_has_reasoning,
        has_resource_id: history_has_resource_id,
        has_tool_call_id: history_has_tool_call_id,
        has_tool_schema: history_has_tool_schema,
        has_input_tokens: history_has_input_tokens,
        has_output_tokens: history_has_output_tokens,
        has_cache_hit_tokens: history_has_cache_hit_tokens,
    };
    let legacy_history_layout = history_exists
        && !legacy_history
        && (!history_has_tool_schema
            || !history_has_input_tokens
            || !history_has_output_tokens
            || !history_has_cache_hit_tokens);
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
    if legacy_history {
        transaction
            .execute(
                "ALTER TABLE history_messages RENAME TO history_messages_legacy",
                [],
            )
            .map_err(schema_error)?;
    }
    if legacy_history_layout {
        transaction
            .execute(
                "ALTER TABLE history_messages RENAME TO history_messages_layout_legacy",
                [],
            )
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
    if legacy_history {
        migrate_history(&transaction)?;
        transaction
            .execute("DROP TABLE history_messages_legacy", [])
            .map_err(schema_error)?;
    }
    if legacy_history_layout {
        migrate_history_layout(&transaction, history_layout)?;
        transaction
            .execute("DROP TABLE history_messages_layout_legacy", [])
            .map_err(schema_error)?;
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

fn migrate_history(transaction: &Transaction<'_>) -> Result<(), MemoryError> {
    let mut statement = transaction
        .prepare(
            "SELECT turn_id, message, created_at_ms FROM history_messages_legacy ORDER BY sequence",
        )
        .map_err(schema_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(schema_error)?;
    for row in rows {
        let (turn_id, encoded, created_at_ms) = row.map_err(schema_error)?;
        let message = serde_json::from_str(&encoded).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "legacy history could not be decoded",
            )
        })?;
        insert_history_message_values(transaction, &turn_id, &message, &[], None, created_at_ms)?;
    }
    Ok(())
}

fn migrate_history_layout(
    transaction: &Transaction<'_>,
    layout: HistoryLayout,
) -> Result<(), MemoryError> {
    let reasoning = if layout.has_reasoning {
        "reasoning"
    } else {
        "NULL"
    };
    let resource_id = if layout.has_resource_id {
        "resource_id"
    } else {
        "NULL"
    };
    let tool_call_id = if layout.has_tool_call_id {
        "tool_call_id"
    } else {
        "NULL"
    };
    let tool_schema = if layout.has_tool_schema {
        "tool_schema"
    } else {
        "'[]'"
    };
    let input_tokens = if layout.has_input_tokens {
        "input_tokens"
    } else {
        "0"
    };
    let output_tokens = if layout.has_output_tokens {
        "output_tokens"
    } else {
        "0"
    };
    let cache_hit_tokens = if layout.has_cache_hit_tokens {
        "cache_hit_tokens"
    } else {
        "0"
    };
    let statement = format!(
        "INSERT INTO history_messages (sequence, turn_id, role, reasoning, content, tool_calls, tool_schema, resource_id, tool_call_id, input_tokens, output_tokens, cache_hit_tokens, created_at_ms) \
         SELECT sequence, turn_id, role, {reasoning}, content, tool_calls, {tool_schema}, {resource_id}, {tool_call_id}, {input_tokens}, {output_tokens}, {cache_hit_tokens}, created_at_ms \
         FROM history_messages_layout_legacy ORDER BY sequence"
    );
    transaction.execute(&statement, []).map_err(schema_error)?;
    Ok(())
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
        .prepare("SELECT sequence, turn_id, role, reasoning, content, tool_calls, tool_schema, resource_id, tool_call_id, input_tokens, output_tokens, cache_hit_tokens, created_at_ms FROM history_messages ORDER BY sequence ASC")
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
            ))
        })
        .map_err(read_error)?;
    rows.map(|row| {
        let (
            sequence,
            turn_id,
            role,
            reasoning,
            content,
            calls,
            schema,
            resource_id,
            call_id,
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            created_at_ms,
        ) = row.map_err(read_error)?;
        let tool_calls = serde_json::from_str(&calls).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "history tool calls could not be decoded",
            )
        })?;
        let tool_schema = serde_json::from_str::<Vec<ToolDefinition>>(&schema).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "history tool schema could not be decoded",
            )
        })?;
        let message = match role.as_str() {
            "user" if tool_schema.is_empty() => Message::User {
                content: content.unwrap_or_default(),
            },
            "assistant" => Message::Assistant {
                reasoning,
                content,
                tool_calls,
            },
            "tool" if tool_calls.is_empty() && tool_schema.is_empty() => Message::Tool {
                resource_id: resource_id
                    .ok_or_else(|| {
                        MemoryError::new(
                            MemoryErrorKind::DecodeFailed,
                            "tool history resource ID is missing",
                        )
                    })?
                    .parse()
                    .map_err(|_| {
                        MemoryError::new(
                            MemoryErrorKind::DecodeFailed,
                            "tool history resource ID is invalid",
                        )
                    })?,
                tool_call_id: call_id.ok_or_else(|| {
                    MemoryError::new(
                        MemoryErrorKind::DecodeFailed,
                        "tool history call ID is missing",
                    )
                })?,
                content: content.unwrap_or_default(),
            },
            "error" if tool_calls.is_empty() && tool_schema.is_empty() => Message::Error {
                message: content.unwrap_or_default(),
            },
            _ => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "history message role is invalid",
                ))
            }
        };
        let usage = if matches!(message, Message::Assistant { .. }) {
            Some(TokenUsage {
                input_tokens: decode_token_count(input_tokens)?,
                output_tokens: decode_token_count(output_tokens)?,
                cache_hit_tokens: decode_token_count(cache_hit_tokens)?,
            })
        } else {
            None
        };
        Ok(HistoryMessage {
            sequence,
            turn_id,
            message,
            tool_schema,
            usage,
            created_at_ms,
        })
    })
    .collect()
}

fn load_token_usage(connection: &Connection) -> Result<TokenUsage, MemoryError> {
    let (input_tokens, output_tokens, cache_hit_tokens) = connection
        .query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cache_hit_tokens), 0) FROM history_messages WHERE role = 'assistant'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )
        .map_err(read_error)?;
    Ok(TokenUsage {
        input_tokens: decode_token_count(input_tokens)?,
        output_tokens: decode_token_count(output_tokens)?,
        cache_hit_tokens: decode_token_count(cache_hit_tokens)?,
    })
}

fn load_last_input_tokens(connection: &Connection) -> Result<u64, MemoryError> {
    let input_tokens = connection
        .query_row(
            "SELECT input_tokens FROM history_messages WHERE role = 'assistant' ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(read_error)?
        .unwrap_or(0);
    decode_token_count(input_tokens)
}

fn decode_token_count(value: i64) -> Result<u64, MemoryError> {
    u64::try_from(value).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::DecodeFailed,
            "history token usage is negative",
        )
    })
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
    message: &Message,
    tool_schema: &[ToolDefinition],
    usage: Option<&TokenUsage>,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    let (role, reasoning, content, tool_calls, resource_id, tool_call_id) = match message {
        Message::User { content } => ("user", None, Some(content.clone()), Vec::new(), None, None),
        Message::Assistant {
            reasoning,
            content,
            tool_calls,
        } => (
            "assistant",
            reasoning.clone(),
            content.clone(),
            tool_calls.clone(),
            None,
            None,
        ),
        Message::Tool {
            resource_id,
            tool_call_id,
            content,
        } => (
            "tool",
            None,
            Some(content.clone()),
            Vec::new(),
            Some(resource_id.to_string()),
            Some(tool_call_id.clone()),
        ),
        Message::Error { message } => {
            ("error", None, Some(message.clone()), Vec::new(), None, None)
        }
        Message::System { .. } => {
            return Err(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "system messages cannot be stored as history",
            ))
        }
    };
    let encoded_calls = serde_json::to_string(&tool_calls).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "history tool calls could not be encoded",
        )
    })?;
    if !matches!(message, Message::Assistant { .. }) && !tool_schema.is_empty() {
        return Err(MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "only assistant history can contain a tool schema",
        ));
    }
    if !matches!(message, Message::Assistant { .. }) && usage.is_some() {
        return Err(MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "only assistant history can contain token usage",
        ));
    }
    let encoded_schema = serde_json::to_string(tool_schema).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "history tool schema could not be encoded",
        )
    })?;
    let usage = usage.cloned().unwrap_or_default();
    let input_tokens = i64::try_from(usage.input_tokens).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "input token usage exceeds SQLite integer range",
        )
    })?;
    let output_tokens = i64::try_from(usage.output_tokens).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "output token usage exceeds SQLite integer range",
        )
    })?;
    let cache_hit_tokens = i64::try_from(usage.cache_hit_tokens).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "cache-hit token usage exceeds SQLite integer range",
        )
    })?;
    transaction.execute("INSERT INTO history_messages (turn_id, role, reasoning, content, tool_calls, tool_schema, resource_id, tool_call_id, input_tokens, output_tokens, cache_hit_tokens, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)", params![turn_id, role, reasoning, content, encoded_calls, encoded_schema, resource_id, tool_call_id, input_tokens, output_tokens, cache_hit_tokens, created_at_ms]).map_err(write_error)?;
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
