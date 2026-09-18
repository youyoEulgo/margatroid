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
pub use types::AgentMemory;

use crate::system::{sync_history_messages_system, sync_history_records_system, sync_settings_system};

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
            .add_system(&self.schedule, sync_settings_system)
            .add_system(&self.schedule, sync_history_records_system);
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
        AgentHistoryMessageWriteRequested, Message,
        ResourceId, TokenUsage, ToolDefinition,
    };
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
    fn history_events_store_user_assistant_and_tool_messages() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("memory.sql");
        let memory = AgentMemory::open(&path).unwrap();
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
                source: "test".to_owned(),
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
        assert!(matches!(
            history[0].message().unwrap(),
            Message::User { .. }
        ));
        assert_eq!(history[0].usage(), None);
        assert_eq!(history[1].usage().as_ref().unwrap().input_tokens, 120);
        assert_eq!(history[1].usage().as_ref().unwrap().output_tokens, 30);
        assert_eq!(history[1].usage().as_ref().unwrap().cache_hit_tokens, 80);
        assert_eq!(history[2].usage(), None);

        assert!(matches!(
            &history[1].message().unwrap(),
            Message::Assistant {
                reasoning: Some(reasoning),
                ..
            } if reasoning == "checking"
        ));
        assert!(matches!(
            history[2].message().unwrap(),
            Message::Tool { .. }
        ));
        assert!(history[0].tool_schema().is_empty());
        assert_eq!(history[1].tool_schema()[0].name, "tool0_read");
        assert!(history[2].tool_schema().is_empty());

        drop(app);
        let restored = AgentMemory::open(&path).unwrap();
        let history = restored.history_messages().unwrap();
        assert_eq!(history[1].usage().unwrap().input_tokens, 120);
    }


}
