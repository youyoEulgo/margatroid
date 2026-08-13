use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{AgentContextMessagesUpdated, AgentHistoryMessageWriteRequested, Message};
use rusqlite::{params, Connection, Transaction};

const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT,
    tool_calls TEXT NOT NULL,
    resource_id TEXT,
    tool_call_id TEXT,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS realtime_messages (
    context TEXT NOT NULL,
    position INTEGER NOT NULL,
    message TEXT NOT NULL,
    PRIMARY KEY (context, position)
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
            .add_system(&self.schedule, sync_realtime_messages_system);
    }
}

pub struct AgentMemory {
    path: PathBuf,
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMessage {
    pub sequence: i64,
    pub turn_id: String,
    pub message: Message,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealtimeContext {
    pub messages: Vec<Message>,
    pub tool_context: Vec<Message>,
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
        let context = load_realtime_messages(&connection)?;
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

        {
            let mut connection = lock_connection(&memory)?;
            let transaction = connection.transaction().map_err(write_error)?;
            rewrite_realtime_messages(&transaction, &context.messages, &context.tool_context)?;
            transaction.commit().map_err(write_error)?;
        }
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
    let legacy_realtime = table_has_column(connection, "realtime_messages", "position")?
        && !table_has_column(connection, "realtime_messages", "context")?;
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
    if history_exists && !legacy_history && !history_has_resource_id {
        transaction
            .execute(
                "ALTER TABLE history_messages ADD COLUMN resource_id TEXT",
                [],
            )
            .map_err(schema_error)?;
    }
    if legacy_history {
        migrate_history(&transaction)?;
        transaction
            .execute("DROP TABLE history_messages_legacy", [])
            .map_err(schema_error)?;
    }
    if legacy_realtime {
        migrate_realtime(&transaction)?;
        transaction
            .execute("DROP TABLE realtime_messages_legacy", [])
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
        insert_history_message_values(transaction, &turn_id, &message, created_at_ms)?;
    }
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
            Message::User { .. } | Message::Assistant { .. } => context.messages.push(message),
            Message::Tool { .. } => context.tool_context.push(message),
            Message::System { .. } => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "legacy realtime context contains a system message",
                ));
            }
        }
    }
    rewrite_realtime_messages(transaction, &context.messages, &context.tool_context)
}

fn schema_error(_: rusqlite::Error) -> MemoryError {
    MemoryError::new(
        MemoryErrorKind::SchemaFailed,
        "memory database schema could not be initialized",
    )
}

fn load_history_messages(connection: &Connection) -> Result<Vec<HistoryMessage>, MemoryError> {
    let mut statement = connection
        .prepare("SELECT sequence, turn_id, role, content, tool_calls, resource_id, tool_call_id, created_at_ms FROM history_messages ORDER BY sequence ASC")
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(read_error)?;
    rows.map(|row| {
        let (sequence, turn_id, role, content, calls, resource_id, call_id, created_at_ms) =
            row.map_err(read_error)?;
        let tool_calls = serde_json::from_str(&calls).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "history tool calls could not be decoded",
            )
        })?;
        let message = match role.as_str() {
            "user" => Message::User {
                content: content.unwrap_or_default(),
                tool_calls,
            },
            "assistant" => Message::Assistant {
                content,
                tool_calls,
            },
            "tool" if tool_calls.is_empty() => Message::Tool {
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
        Ok(HistoryMessage {
            sequence,
            turn_id,
            message,
            created_at_ms,
        })
    })
    .collect()
}

fn load_realtime_messages(connection: &Connection) -> Result<RealtimeContext, MemoryError> {
    let mut statement = connection.prepare("SELECT context, position, message FROM realtime_messages ORDER BY context ASC, position ASC").map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(read_error)?;
    let mut context = RealtimeContext::default();
    let mut conversation_position = 0i64;
    let mut tool_position = 0i64;
    for row in rows {
        let (kind, position, encoded) = row.map_err(read_error)?;
        let message = serde_json::from_str::<Message>(&encoded).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "realtime message JSON could not be decoded",
            )
        })?;
        let expected = if kind == "conversation" {
            &mut conversation_position
        } else if kind == "tool" {
            &mut tool_position
        } else {
            return Err(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "realtime context is invalid",
            ));
        };
        if position != *expected {
            return Err(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "realtime message positions are not continuous",
            ));
        }
        *expected += 1;
        match kind.as_str() {
            "conversation"
                if matches!(message, Message::User { .. } | Message::Assistant { .. }) =>
            {
                context.messages.push(message)
            }
            "tool" if matches!(message, Message::Tool { .. }) => context.tool_context.push(message),
            _ => {
                return Err(MemoryError::new(
                    MemoryErrorKind::DecodeFailed,
                    "realtime message type does not match context",
                ))
            }
        }
    }
    Ok(context)
}

fn rewrite_realtime_messages(
    transaction: &Transaction<'_>,
    messages: &[Message],
    tool_context: &[Message],
) -> Result<(), MemoryError> {
    if messages
        .iter()
        .any(|m| !matches!(m, Message::User { .. } | Message::Assistant { .. }))
        || tool_context
            .iter()
            .any(|m| !matches!(m, Message::Tool { .. }))
    {
        return Err(MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "realtime message type is invalid",
        ));
    }
    transaction
        .execute("DELETE FROM realtime_messages", [])
        .map_err(write_error)?;
    for (context, entries) in [("conversation", messages), ("tool", tool_context)] {
        for (position, message) in entries.iter().enumerate() {
            let encoded = serde_json::to_string(message).map_err(|_| {
                MemoryError::new(
                    MemoryErrorKind::WriteFailed,
                    "realtime message JSON could not be encoded",
                )
            })?;
            transaction.execute("INSERT INTO realtime_messages (context, position, message) VALUES (?1, ?2, ?3)", params![context, position as i64, encoded]).map_err(write_error)?;
        }
    }
    Ok(())
}

fn insert_history_message(
    transaction: &Transaction<'_>,
    event: &AgentHistoryMessageWriteRequested,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    insert_history_message_values(transaction, &event.id, &event.message, created_at_ms)
}

fn insert_history_message_values(
    transaction: &Transaction<'_>,
    turn_id: &str,
    message: &Message,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    let (role, content, tool_calls, resource_id, tool_call_id) = match message {
        Message::User {
            content,
            tool_calls,
        } => (
            "user",
            Some(content.clone()),
            tool_calls.clone(),
            None,
            None,
        ),
        Message::Assistant {
            content,
            tool_calls,
        } => ("assistant", content.clone(), tool_calls.clone(), None, None),
        Message::Tool {
            resource_id,
            tool_call_id,
            content,
        } => (
            "tool",
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
    transaction.execute("INSERT INTO history_messages (turn_id, role, content, tool_calls, resource_id, tool_call_id, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![turn_id, role, content, encoded_calls, resource_id, tool_call_id, created_at_ms]).map_err(write_error)?;
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

fn sync_realtime_messages_system(world: &mut World) {
    let events = world
        .event_reader::<AgentContextMessagesUpdated>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let result = sync_realtime_message(world, &event);
        if let Err(error) = result {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

fn sync_realtime_message(
    world: &World,
    event: &AgentContextMessagesUpdated,
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
    rewrite_realtime_messages(&transaction, &event.messages, &event.tool_context)?;
    transaction.commit().map_err(write_error)
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
                tool_calls: Vec::new(),
            }],
            tool_context: vec![Message::Tool {
                resource_id: ResourceId::parse("tool:local/test:latest").unwrap(),
                tool_call_id: "call-1".into(),
                content: "tool output".into(),
            }],
        };
        app.world_mut()
            .bind_agent_memory(agent, memory, &context)
            .unwrap();

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
        for message in [
            Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
            Message::Assistant {
                content: None,
                tool_calls: Vec::new(),
            },
            Message::Tool {
                resource_id: ResourceId::parse("tool:local/test:latest").unwrap(),
                tool_call_id: "call-1".into(),
                content: "tool output".into(),
            },
        ] {
            app.world().emit_event(AgentHistoryMessageWriteRequested {
                id: "turn-1".into(),
                agent,
                message,
            });
        }
        app.tick();

        let memory = app.world().get_component::<AgentMemory>(agent).unwrap();
        let history = memory.history_messages().unwrap();
        assert_eq!(history.len(), 3);
        assert!(matches!(history[0].message, Message::User { .. }));
        assert!(matches!(history[1].message, Message::Assistant { .. }));
        assert!(matches!(history[2].message, Message::Tool { .. }));
    }

    #[test]
    fn failed_realtime_rewrite_preserves_the_previous_snapshot() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        let original = RealtimeContext {
            messages: vec![Message::User {
                content: "keep".into(),
                tool_calls: Vec::new(),
            }],
            tool_context: Vec::new(),
        };
        app.world_mut()
            .bind_agent_memory(agent, memory, &original)
            .unwrap();
        app.world().emit_event(AgentContextMessagesUpdated {
            agent,
            messages: vec![Message::System {
                content: "invalid".into(),
            }],
            tool_context: Vec::new(),
        });

        app.tick();

        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(restored, original);
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
    fn memory_requires_runtime_schedule() {
        let result = std::panic::catch_unwind(|| {
            App::new().add_plugin(MemoryPlugin::default());
        });
        assert!(result.is_err());
    }
}
