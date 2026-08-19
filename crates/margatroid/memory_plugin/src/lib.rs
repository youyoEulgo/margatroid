use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{
    AgentHistoryMessageWriteRequested, AgentRealtimeContextReadCompleted,
    AgentRealtimeContextReadRequested, AgentRealtimeContextWriteRequested, AgentRealtimeMessage,
    Message, TokenUsage, ToolDefinition,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryErrorKind {
    InvalidPath,
    DirectoryCreateFailed,
    OpenFailed,
    SchemaFailed,
    ReadFailed,
    DecodeFailed,
    AgentNotAlive,
    AgentMemoryMissing,
    AlreadyBound,
    PluginMissing,
    WriteFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryError {
    kind: MemoryErrorKind,
    message: String,
}

impl MemoryError {
    fn new(kind: MemoryErrorKind, message: impl Into<String>) -> Self {
        const MAX_MESSAGE_BYTES: usize = 512;
        const SUFFIX: &str = "...";

        let mut message = message.into();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut boundary = MAX_MESSAGE_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> MemoryErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for MemoryError {}

pub struct MemoryPlugin {
    schedule: String,
}

impl MemoryPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for MemoryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MemoryPluginInstalled;

impl Resource for MemoryPluginInstalled {}

impl Plugin for MemoryPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<MemoryPluginInstalled>() {
            panic!("MemoryPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("MemoryPlugin schedule does not exist");
        }

        app.world_mut().insert_resource(MemoryPluginInstalled);
        app.add_system(&self.schedule, sync_history_messages_system)
            .add_system(&self.schedule, read_realtime_context_system)
            .add_system(&self.schedule, sync_realtime_context_system);
    }
}

pub struct AgentMemory {
    path: PathBuf,
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryMessage {
    pub sequence: i64,
    pub turn_id: String,
    pub message: Message,
    pub tool_schema: Vec<ToolDefinition>,
    pub usage: Option<TokenUsage>,
    pub created_at_ms: i64,
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

impl Component for AgentMemory {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMemoryWriteFailed {
    pub agent: Entity,
    pub error: MemoryError,
}

impl Event for AgentMemoryWriteFailed {}

pub trait WorldMemoryExt {
    fn bind_agent_memory(
        &mut self,
        agent: Entity,
        memory: AgentMemory,
        context: &RealtimeContext,
    ) -> Result<(), MemoryError>;
}

impl WorldMemoryExt for World {
    fn bind_agent_memory(
        &mut self,
        agent: Entity,
        memory: AgentMemory,
        context: &RealtimeContext,
    ) -> Result<(), MemoryError> {
        require_plugin(self)?;
        if !self.is_alive(agent) {
            return Err(MemoryError::new(
                MemoryErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ));
        }
        if self.contains_component::<AgentMemory>(agent) {
            return Err(MemoryError::new(
                MemoryErrorKind::AlreadyBound,
                "agent already has memory",
            ));
        }

        let _ = context;
        if !self.insert_component(agent, memory) {
            return Err(MemoryError::new(
                MemoryErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ));
        }
        Ok(())
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

fn require_plugin(world: &World) -> Result<(), MemoryError> {
    if world.contains_resource::<MemoryPluginInstalled>() {
        Ok(())
    } else {
        Err(MemoryError::new(
            MemoryErrorKind::PluginMissing,
            "MemoryPlugin is not installed",
        ))
    }
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
            Message::System { .. } => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "legacy realtime context contains a system message",
                ));
            }
        }
    }
    rewrite_realtime_context(
        transaction,
        &context
            .ordered_messages
            .into_iter()
            .map(|message| AgentRealtimeMessage {
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

fn load_ordered_realtime_messages(
    connection: &Connection,
) -> Result<Vec<AgentRealtimeMessage>, MemoryError> {
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
        messages.push(AgentRealtimeMessage { message, usage });
    }
    Ok(messages)
}

fn rewrite_realtime_context(
    transaction: &Transaction<'_>,
    ordered_messages: &[AgentRealtimeMessage],
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

fn insert_history_message(
    transaction: &Transaction<'_>,
    event: &AgentHistoryMessageWriteRequested,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    insert_history_message_values(
        transaction,
        &event.id,
        &event.message,
        &event.tool_schema,
        event.usage.as_ref(),
        created_at_ms,
    )
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

fn sync_realtime_context_system(world: &mut World) {
    let events = world
        .event_reader::<AgentRealtimeContextWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let result = sync_realtime_context(world, &event);
        if let Err(error) = result {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

fn sync_realtime_context(
    world: &World,
    event: &AgentRealtimeContextWriteRequested,
) -> Result<(), MemoryError> {
    if !world.is_alive(event.agent) {
        return Err(MemoryError::new(
            MemoryErrorKind::AgentNotAlive,
            "agent entity is not alive",
        ));
    }
    let memory = world
        .get_component::<AgentMemory>(event.agent)
        .ok_or_else(|| {
            MemoryError::new(
                MemoryErrorKind::AgentMemoryMissing,
                "agent does not have memory",
            )
        })?;
    let mut connection = lock_connection(memory)?;
    let transaction = connection.transaction().map_err(write_error)?;
    rewrite_realtime_context(&transaction, &event.messages)?;
    transaction.commit().map_err(write_error)
}

fn read_realtime_context_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentRealtimeContextReadRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = (|| {
            let memory = world
                .get_component::<AgentMemory>(request.agent)
                .ok_or_else(|| {
                    MemoryError::new(
                        MemoryErrorKind::AgentMemoryMissing,
                        "agent does not have memory",
                    )
                })?;
            let connection = lock_connection(memory)?;
            Ok(load_ordered_realtime_messages(&connection)?)
        })();
        world.emit_event(AgentRealtimeContextReadCompleted {
            id: request.id,
            agent: request.agent,
            result: result.map_err(|error: MemoryError| error.to_string()),
        });
    }
}

fn sync_history_messages_system(world: &mut World) {
    let events = world
        .event_reader::<AgentHistoryMessageWriteRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let result = sync_history_message(world, &event);
        if let Err(error) = result {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

fn sync_history_message(
    world: &World,
    event: &AgentHistoryMessageWriteRequested,
) -> Result<(), MemoryError> {
    if !world.is_alive(event.agent) {
        return Err(MemoryError::new(
            MemoryErrorKind::AgentNotAlive,
            "agent entity is not alive",
        ));
    }
    let memory = world
        .get_component::<AgentMemory>(event.agent)
        .ok_or_else(|| {
            MemoryError::new(
                MemoryErrorKind::AgentMemoryMissing,
                "agent does not have memory",
            )
        })?;
    let mut connection = lock_connection(memory)?;
    let transaction = connection.transaction().map_err(write_error)?;
    insert_history_message(&transaction, event, current_unix_milliseconds()?)?;
    transaction.commit().map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_plugin::App;
    use margatroid_types::ResourceId;
    use tempfile::tempdir;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(MemoryPlugin::default());
        app
    }

    #[test]
    fn open_creates_schema_and_restores_realtime_messages() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        let context = RealtimeContext {
            messages: vec![Message::User {
                content: "restored".into(),
            }],
            tool_context: vec![Message::Tool {
                resource_id: ResourceId::parse("tool:local/test:latest").unwrap(),
                tool_call_id: "call-1".into(),
                content: "tool output".into(),
            }],
            ordered_messages: vec![
                Message::User {
                    content: "restored".into(),
                },
                Message::Tool {
                    resource_id: ResourceId::parse("tool:local/test:latest").unwrap(),
                    tool_call_id: "call-1".into(),
                    content: "tool output".into(),
                },
            ],
            token_usage: TokenUsage::default(),
            last_input_tokens: 0,
        };
        app.world_mut()
            .bind_agent_memory(agent, memory, &context)
            .unwrap();
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: context
                .ordered_messages
                .iter()
                .cloned()
                .map(|message| AgentRealtimeMessage {
                    message,
                    usage: None,
                })
                .collect(),
        });
        app.tick();

        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(restored, context);
    }

    #[test]
    fn history_events_store_user_assistant_and_tool_messages() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .bind_agent_memory(agent, memory, &RealtimeContext::default())
            .unwrap();
        for (index, message) in [
            Message::User {
                content: "hello".into(),
            },
            Message::Assistant {
                reasoning: Some("checking".into()),
                content: None,
                tool_calls: Vec::new(),
            },
            Message::Tool {
                resource_id: ResourceId::parse("tool:local/test:latest").unwrap(),
                tool_call_id: "call-1".into(),
                content: "tool output".into(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let tool_schema = if index == 1 {
                vec![ToolDefinition {
                    name: "tool0_read".into(),
                    description: "Read a file.".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                }]
            } else {
                Vec::new()
            };
            app.world().emit_event(AgentHistoryMessageWriteRequested {
                id: "turn-1".into(),
                agent,
                message,
                tool_schema,
                usage: (index == 1).then_some(TokenUsage {
                    input_tokens: 120,
                    output_tokens: 30,
                    cache_hit_tokens: 80,
                }),
            });
        }
        app.tick();

        let memory = app.world().get_component::<AgentMemory>(agent).unwrap();
        let history = memory.history_messages().unwrap();
        assert_eq!(history.len(), 3);
        assert!(matches!(history[0].message, Message::User { .. }));
        assert_eq!(history[0].usage, None);
        assert_eq!(history[1].usage.as_ref().unwrap().input_tokens, 120);
        assert_eq!(history[1].usage.as_ref().unwrap().output_tokens, 30);
        assert_eq!(history[1].usage.as_ref().unwrap().cache_hit_tokens, 80);
        assert_eq!(history[2].usage, None);

        assert!(matches!(
            &history[1].message,
            Message::Assistant {
                reasoning: Some(reasoning),
                ..
            } if reasoning == "checking"
        ));
        assert!(matches!(history[2].message, Message::Tool { .. }));
        assert!(history[0].tool_schema.is_empty());
        assert_eq!(history[1].tool_schema[0].name, "tool0_read");
        assert!(history[2].tool_schema.is_empty());

        drop(app);
        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(
            restored.token_usage,
            TokenUsage {
                input_tokens: 120,
                output_tokens: 30,
                cache_hit_tokens: 80,
            }
        );
        assert_eq!(restored.last_input_tokens, 120);
    }

    #[test]
    fn realtime_effect_replaces_the_previous_snapshot() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        let original = RealtimeContext {
            messages: vec![Message::User {
                content: "keep".into(),
            }],
            tool_context: Vec::new(),
            ordered_messages: vec![Message::User {
                content: "keep".into(),
            }],
            token_usage: TokenUsage::default(),
            last_input_tokens: 0,
        };
        app.world_mut()
            .bind_agent_memory(agent, memory, &original)
            .unwrap();
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: vec![AgentRealtimeMessage {
                message: Message::User {
                    content: "keep".into(),
                },
                usage: None,
            }],
        });
        app.tick();
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: vec![AgentRealtimeMessage {
                message: Message::User {
                    content: "new".into(),
                },
                usage: None,
            }],
        });

        app.tick();

        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(
            restored.ordered_messages,
            vec![Message::User {
                content: "new".into()
            }]
        );
    }

    #[test]
    fn open_migrates_the_previous_message_schema() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE history_messages (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    turn_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    message TEXT NOT NULL,
                    resources TEXT NOT NULL DEFAULT '[]',
                    created_at_ms INTEGER NOT NULL
                );
                CREATE TABLE realtime_messages (
                    position INTEGER PRIMARY KEY,
                    message TEXT NOT NULL
                );",
            )
            .unwrap();
        let message = r#"{"User":{"content":"legacy"}}"#;
        connection
            .execute(
                "INSERT INTO history_messages (turn_id, role, message, created_at_ms) VALUES ('turn-1', 'user', ?1, 1)",
                [message],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO realtime_messages (position, message) VALUES (0, ?1)",
                [message],
            )
            .unwrap();
        drop(connection);

        let (memory, context) = AgentMemory::open(&path).unwrap();
        assert_eq!(context.messages.len(), 1);
        assert!(context.tool_context.is_empty());
        assert_eq!(memory.history_messages().unwrap().len(), 1);
    }

    #[test]
    fn open_rebuilds_the_split_history_schema_in_canonical_order() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE history_messages (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    turn_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT,
                    tool_calls TEXT NOT NULL,
                    resource_id TEXT,
                    tool_call_id TEXT,
                    created_at_ms INTEGER NOT NULL,
                    reasoning TEXT,
                    tool_schema TEXT NOT NULL
                );
                CREATE TABLE realtime_messages (
                    context TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    message TEXT NOT NULL,
                    PRIMARY KEY (context, position)
                );
                INSERT INTO history_messages
                    (turn_id, role, content, tool_calls, created_at_ms, reasoning, tool_schema)
                    VALUES (
                        'turn-1',
                        'assistant',
                        'legacy answer',
                        '[]',
                        1,
                        'legacy thought',
                        '[{"name":"tool0_read","description":"Read a file.","input_schema":{"type":"object"}}]'
                    );"#,
            )
            .unwrap();
        drop(connection);

        let (memory, _) = AgentMemory::open(&path).unwrap();
        let history = memory.history_messages().unwrap();

        assert!(matches!(
            &history[0].message,
            Message::Assistant {
                reasoning: Some(reasoning),
                content: Some(content),
                ..
            } if reasoning == "legacy thought" && content == "legacy answer"
        ));
        assert_eq!(history[0].tool_schema[0].name, "tool0_read");
        assert_eq!(history[0].usage, Some(TokenUsage::default()));

        let connection = Connection::open(&path).unwrap();
        let mut statement = connection
            .prepare("PRAGMA table_info(history_messages)")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "sequence",
                "turn_id",
                "role",
                "reasoning",
                "content",
                "tool_calls",
                "tool_schema",
                "resource_id",
                "tool_call_id",
                "input_tokens",
                "output_tokens",
                "cache_hit_tokens",
                "created_at_ms",
            ]
        );
    }

    #[test]
    fn memory_requires_runtime_schedule() {
        let result = std::panic::catch_unwind(|| {
            App::new().add_plugin(MemoryPlugin::default());
        });
        assert!(result.is_err());
    }
}
