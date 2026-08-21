use std::sync::Arc;

use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use margatroid_types::{AgentHistoryMessageWriteRequested, Message};
use memory_plugin::{AgentMemory, MemoryPlugin};
use tempfile::tempdir;

fn attach_agent(app: &mut App, memory: AgentMemory) -> core_plugin::Entity {
    let world = app.world_mut();
    let entity = world.spawn();
    let workspace = world.spawn();
    let image = world.spawn();
    let model = agent_plugin::AgentModelInfo {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        context_window_tokens: 1024,
    };
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    world.insert_component(
        entity,
        agent_plugin::Agent {
            info: agent_plugin::AgentInfo {
                image_entity: image,
                workspace_id: workspace,
                model: model.clone(),
                project_root: Default::default(),
                image_root: Default::default(),
                home_root: Default::default(),
                image_dependencies: Default::default(),
                image_sources: Default::default(),
            },
            creation: agent_plugin::AgentCreationState {
                request_id: "test".to_owned(),
                reply: agent_plugin::AgentCreateReply::new(sender),
                initialization: Default::default(),
            },
            mcl: Default::default(),
            resources: Default::default(),
            memory: agent_plugin::AgentMemoryHandle::new(Arc::new(memory)),
            inference: agent_plugin::AgentInferenceState {
                model,
                pending: Default::default(),
            },
            tools: Default::default(),
            lua: Default::default(),
            lifecycle: agent_plugin::AgentLifecycleState::Running,
            turn: Default::default(),
            token_usage: Default::default(),
            last_error: None,
        },
    );
    entity
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("memory.sql");
    let (memory, restored) = AgentMemory::open(&path).unwrap();
    assert!(restored.messages.is_empty());
    assert!(restored.tool_context.is_empty());

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(MemoryPlugin::default());
    let agent = attach_agent(&mut app, memory);
    app.world().emit_event(AgentHistoryMessageWriteRequested {
        id: "turn-1".into(),
        agent,
        message: Message::User {
            content: "hello".into(),
        },
        tool_schema: Vec::new(),
        usage: None,
    });
    app.tick();

    let history = app
        .world()
        .get_component::<agent_plugin::Agent>(agent)
        .unwrap()
        .memory
        .history_messages()
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].turn_id, "turn-1");
    assert_eq!(
        history[0].message,
        Message::User {
            content: "hello".into(),
        }
    );
}
