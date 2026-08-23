mod error;
mod events;
mod handler;
mod system;
mod types;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, Plugin, Resource};

pub use agent_plugin::HistoryMessage;
pub use error::{MemoryError, MemoryErrorKind};
pub use events::AgentMemoryWriteFailed;
pub use types::{AgentMemory, RealtimeContext};

use crate::system::{
    read_realtime_context_system, sync_history_messages_system, sync_realtime_context_system,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_plugin::{
        Agent, AgentCreateReply, AgentCreationState, AgentInferenceState, AgentInfo,
        AgentLifecycleState, AgentMemoryHandle, AgentModelInfo,
    };
    use app_runtime_plugin::RuntimePlugin;
    use core_plugin::{App, Entity, World};
    use margatroid_types::{
        AgentHistoryMessageWriteRequested, AgentRealtimeContextWriteRequested, MclMessage, Message,
        ResourceId, TokenUsage, ToolDefinition,
    };
    use rusqlite::Connection;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(MemoryPlugin::default());
        app
    }

    fn attach_agent(world: &mut World, memory: AgentMemory) -> Entity {
        let entity = world.spawn();
        let workspace = world.spawn();
        let image = world.spawn();
        let model = AgentModelInfo {
            provider: "test".to_owned(),
            model: "test".to_owned(),
            context_window_tokens: 1024,
        };
        let (sender, _receiver) = tokio::sync::oneshot::channel();
        world.insert_component(
            entity,
            Agent {
                info: AgentInfo {
                    image_entity: image,
                    workspace_id: workspace,
                    model: model.clone(),
                    project_root: Default::default(),
                    image_root: Default::default(),
                    home_root: Default::default(),
                    image_dependencies: Default::default(),
                    image_sources: Default::default(),
                },
                creation: AgentCreationState {
                    request_id: "test".to_owned(),
                    reply: AgentCreateReply::new(sender),
                    initialization: Default::default(),
                },
                mcl: Default::default(),
                resources: Default::default(),
                memory: AgentMemoryHandle::new(Arc::new(memory)),
                inference: AgentInferenceState {
                    model,
                    pending: Default::default(),
                },
                tools: Default::default(),
                lua: Default::default(),
                lifecycle: AgentLifecycleState::Running,
                turn: Default::default(),
                token_usage: Default::default(),
                last_error: None,
            },
        );
        entity
    }

    #[test]
    fn open_creates_schema_and_restores_realtime_messages() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let (memory, _) = AgentMemory::open(&path).unwrap();
        let mut app = test_app();
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
        let agent = attach_agent(app.world_mut(), memory);
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: context
                .ordered_messages
                .iter()
                .cloned()
                .map(|message| MclMessage {
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
        let agent = attach_agent(app.world_mut(), memory);
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

        let memory = app.world().get_component::<Agent>(agent).unwrap();
        let history = memory.memory.history_messages().unwrap();
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
        let agent = attach_agent(app.world_mut(), memory);
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: vec![MclMessage {
                message: Message::User {
                    content: "keep".into(),
                },
                usage: None,
            }],
        });
        app.tick();
        app.world().emit_event(AgentRealtimeContextWriteRequested {
            agent,
            messages: vec![MclMessage {
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
