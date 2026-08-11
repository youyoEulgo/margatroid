use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use margatroid_types::{AgentHistoryMessageWriteRequested, Message};
use memory_plugin::{AgentMemory, MemoryPlugin, WorldMemoryExt};
use tempfile::tempdir;

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
    let agent = app.world_mut().spawn();
    app.world_mut()
        .bind_agent_memory(agent, memory, &restored)
        .unwrap();
    app.world().emit_event(AgentHistoryMessageWriteRequested {
        id: "turn-1".into(),
        agent,
        message: Message::User {
            content: "hello".into(),
            tool_calls: Vec::new(),
        },
    });
    app.tick();

    assert_eq!(
        app.world()
            .get_component::<AgentMemory>(agent)
            .unwrap()
            .path(),
        path
    );
    let history = app
        .world()
        .get_component::<AgentMemory>(agent)
        .unwrap()
        .history_messages()
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].turn_id, "turn-1");
    assert_eq!(
        history[0].message,
        Message::User {
            content: "hello".into(),
            tool_calls: Vec::new(),
        }
    );
}
