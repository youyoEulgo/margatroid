use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use margatroid_types::{
    AgentContextMessagesUpdated, AgentMessage, AgentResourcesUsed, Message, MessageResource,
    ResourceRef,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id TEXT NOT NULL,
    role TEXT NOT NULL,
    message TEXT NOT NULL,
    resources TEXT NOT NULL DEFAULT '[]',
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS realtime_messages (
    position INTEGER PRIMARY KEY,
    message TEXT NOT NULL
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

struct MemoryPluginInstalled;

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
        app.add_system(&self.schedule, sync_realtime_messages_system)
            .add_system(&self.schedule, sync_history_resources_system);
    }
}

pub struct AgentMemory {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl AgentMemory {
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Vec<Message>), MemoryError> {
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
        let connection = Connection::open(&path).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::OpenFailed,
                "memory database could not be opened",
            )
        })?;
        initialize_schema(&connection)?;
        let messages = load_realtime_messages(&connection)?;
        Ok((
            Self {
                path,
                connection: Mutex::new(connection),
            },
            messages,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
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
        messages: &[Message],
    ) -> Result<(), MemoryError>;

    fn append_history_message(&mut self, event: &AgentMessage) -> Result<(), MemoryError>;
}

impl WorldMemoryExt for World {
    fn bind_agent_memory(
        &mut self,
        agent: Entity,
        memory: AgentMemory,
        messages: &[Message],
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
            rewrite_realtime_messages(&transaction, messages)?;
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

    fn append_history_message(&mut self, event: &AgentMessage) -> Result<(), MemoryError> {
        if matches!(event.message, Message::Tool { .. }) {
            return Ok(());
        }
        if matches!(event.message, Message::System { .. }) {
            return Err(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "system messages cannot be stored as agent history",
            ));
        }
        require_plugin(self)?;
        if !self.is_alive(event.agent) {
            return Err(MemoryError::new(
                MemoryErrorKind::AgentNotAlive,
                "agent entity is not alive",
            ));
        }
        let memory = self
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

fn initialize_schema(connection: &Connection) -> Result<(), MemoryError> {
    connection.execute_batch(HISTORY_SCHEMA).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::SchemaFailed,
            "memory database schema could not be initialized",
        )
    })
}

fn load_realtime_messages(connection: &Connection) -> Result<Vec<Message>, MemoryError> {
    let mut statement = connection
        .prepare("SELECT position, message FROM realtime_messages ORDER BY position ASC")
        .map_err(read_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(read_error)?;
    let mut messages = Vec::new();
    for (expected, row) in rows.enumerate() {
        let (position, encoded) = row.map_err(read_error)?;
        if position != expected as i64 {
            return Err(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "realtime message positions are not continuous",
            ));
        }
        let message = serde_json::from_str::<Message>(&encoded).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "realtime message JSON could not be decoded",
            )
        })?;
        if matches!(message, Message::System { .. }) {
            return Err(MemoryError::new(
                MemoryErrorKind::DecodeFailed,
                "system messages cannot be restored into dynamic context",
            ));
        }
        messages.push(message);
    }
    Ok(messages)
}

fn rewrite_realtime_messages(
    transaction: &Transaction<'_>,
    messages: &[Message],
) -> Result<(), MemoryError> {
    if messages
        .iter()
        .any(|message| matches!(message, Message::System { .. }))
    {
        return Err(MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "system messages cannot be stored in dynamic context",
        ));
    }
    transaction
        .execute("DELETE FROM realtime_messages", [])
        .map_err(write_error)?;
    for (position, message) in messages.iter().enumerate() {
        let encoded = serde_json::to_string(message).map_err(|_| {
            MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "realtime message JSON could not be encoded",
            )
        })?;
        transaction
            .execute(
                "INSERT INTO realtime_messages (position, message) VALUES (?1, ?2)",
                params![position as i64, encoded],
            )
            .map_err(write_error)?;
    }
    Ok(())
}

fn insert_history_message(
    transaction: &Transaction<'_>,
    event: &AgentMessage,
    created_at_ms: i64,
) -> Result<(), MemoryError> {
    let role = match &event.message {
        Message::User { .. } => "user",
        Message::Assistant { .. } => "assistant",
        Message::Tool { .. } | Message::System { .. } => {
            return Err(MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "message type cannot be stored as agent history",
            ));
        }
    };
    let encoded = serde_json::to_string(&event.message).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "history message JSON could not be encoded",
        )
    })?;
    transaction
        .execute(
            "INSERT INTO history_messages
             (turn_id, role, message, resources, created_at_ms)
             VALUES (?1, ?2, ?3, '[]', ?4)",
            params![event.id, role, encoded, created_at_ms],
        )
        .map_err(write_error)?;
    Ok(())
}

fn merge_history_resources(
    transaction: &Transaction<'_>,
    turn_id: &str,
    resources: &[MessageResource],
) -> Result<(), MemoryError> {
    if resources.is_empty() {
        return Ok(());
    }
    let row = transaction
        .query_row(
            "SELECT sequence, resources FROM history_messages
             WHERE turn_id = ?1 AND role = 'user'
             ORDER BY sequence DESC LIMIT 1",
            params![turn_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(read_error)?
        .ok_or_else(|| {
            MemoryError::new(
                MemoryErrorKind::WriteFailed,
                "user history message for resource usage was not found",
            )
        })?;
    let mut merged = serde_json::from_str::<Vec<ResourceRef>>(&row.1).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::DecodeFailed,
            "history resource JSON could not be decoded",
        )
    })?;
    for resource in resources {
        if !merged.contains(resource) {
            merged.push(resource.clone());
        }
    }
    let encoded = serde_json::to_string(&merged).map_err(|_| {
        MemoryError::new(
            MemoryErrorKind::WriteFailed,
            "history resource JSON could not be encoded",
        )
    })?;
    transaction
        .execute(
            "UPDATE history_messages SET resources = ?1 WHERE sequence = ?2",
            params![encoded, row.0],
        )
        .map_err(write_error)?;
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
    rewrite_realtime_messages(&transaction, &event.messages)?;
    transaction.commit().map_err(write_error)
}

fn sync_history_resources_system(world: &mut World) {
    let events = world
        .event_reader::<AgentResourcesUsed>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let result = sync_history_resources(world, &event);
        if let Err(error) = result {
            world.emit_event(AgentMemoryWriteFailed {
                agent: event.agent,
                error,
            });
        }
    }
}

fn sync_history_resources(world: &World, event: &AgentResourcesUsed) -> Result<(), MemoryError> {
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
    merge_history_resources(&transaction, &event.id, &event.resources)?;
    transaction.commit().map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_plugin::App;
    use margatroid_types::{AgentContextMessagesUpdated, MessageIntent};
    use tempfile::tempdir;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(MemoryPlugin::default());
        app
    }

    fn user_event(agent: Entity, id: &str) -> AgentMessage {
        AgentMessage {
            id: id.into(),
            agent,
            message: Message::User {
                content: "hello".into(),
            },
            intent: MessageIntent::UserWithoutToolCalls,
        }
    }

    #[test]
    fn open_creates_schema_and_restores_realtime_messages() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        let messages = vec![Message::User {
            content: "restored".into(),
        }];
        app.world_mut()
            .bind_agent_memory(agent, memory, &messages)
            .unwrap();

        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(restored, messages);
    }

    #[test]
    fn tool_messages_skip_history_but_sync_realtime_context() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .bind_agent_memory(agent, memory, &[])
            .unwrap();
        app.world_mut()
            .append_history_message(&user_event(agent, "turn-1"))
            .unwrap();
        app.world_mut()
            .append_history_message(&AgentMessage {
                id: "turn-1".into(),
                agent,
                message: Message::Tool {
                    tool_call_id: "call-1".into(),
                    content: "tool output".into(),
                },
                intent: MessageIntent::ResolveToolCall,
            })
            .unwrap();

        app.world().emit_event(AgentContextMessagesUpdated {
            agent,
            messages: vec![
                Message::User {
                    content: "hello".into(),
                },
                Message::Tool {
                    tool_call_id: "call-1".into(),
                    content: "tool output".into(),
                },
            ],
        });
        app.tick();

        let connection = Connection::open(&path).unwrap();
        let history_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM history_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        let realtime_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM realtime_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(history_count, 1);
        assert_eq!(realtime_count, 2);
    }

    #[test]
    fn resource_usage_is_merged_into_user_history() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        app.world_mut()
            .bind_agent_memory(agent, memory, &[])
            .unwrap();
        app.world_mut()
            .append_history_message(&user_event(agent, "turn-1"))
            .unwrap();
        let resource = ResourceRef::new(
            "skill",
            margatroid_types::ResourceName::new("local/review").unwrap(),
        )
        .unwrap();
        app.world().emit_event(AgentResourcesUsed {
            id: "turn-1".into(),
            agent,
            resources: vec![resource.clone(), resource],
        });
        app.tick();

        let connection = Connection::open(&path).unwrap();
        let encoded: String = connection
            .query_row("SELECT resources FROM history_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        let resources: Vec<ResourceRef> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(resources.len(), 1);
    }

    #[test]
    fn failed_realtime_rewrite_preserves_the_previous_snapshot() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
        let agent = app.world_mut().spawn();
        let original = vec![Message::User {
            content: "keep".into(),
        }];
        app.world_mut()
            .bind_agent_memory(agent, memory, &original)
            .unwrap();
        app.world().emit_event(AgentContextMessagesUpdated {
            agent,
            messages: vec![Message::System {
                content: "invalid".into(),
            }],
        });

        app.tick();

        let (_, restored) = AgentMemory::open(&path).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn memory_requires_runtime_schedule() {
        let result = std::panic::catch_unwind(|| {
            App::new().add_plugin(MemoryPlugin::default());
        });
        assert!(result.is_err());
    }
}
