use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::{AgentMessage, Message, ResourceId, ToolCall};
use skill_plugin::SkillPlugin;
use tempfile::tempdir;
use tool_plugin::{AgentToolEnvironment, ToolCallRequest, ToolPlugin};

#[test]
fn skill_loader_reads_skill_and_returns_tool_message() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    let path = home.path().join("local/review/latest/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "Review the requested change.").unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(SkillPlugin::open(home.path()).unwrap());
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );
    let resource = ResourceId::parse("skill:local/review:latest").unwrap();
    app.world().send_event(ToolCallRequest {
        id: "turn-1".into(),
        agent,
        call: ToolCall {
            id: "call-1".into(),
            resource,
            arguments: "{}".into(),
        },
    });
    for _ in 0..4 {
        app.tick();
        if let Some(message) = app
            .world()
            .event_reader::<AgentMessage>()
            .into_iter()
            .next()
        {
            assert_eq!(
                message.message,
                Message::Tool {
                    tool_call_id: "call-1".into(),
                    content: "Review the requested change.".into()
                }
            );
            return;
        }
    }
    panic!("skill tool response was not emitted");
}
