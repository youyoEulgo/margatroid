use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::{AgentMessage, Message, ResourceId, ToolCall};
use tempfile::tempdir;
use tool_plugin::{AgentToolEnvironment, ToolCallRequest, ToolPlugin};
use workflow_plugin::WorkflowPlugin;

#[test]
fn workflow_loader_returns_placeholder_tool_message() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("local/review/latest")).unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(WorkflowPlugin::open(home.path()).unwrap());
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );
    let resource = ResourceId::parse("workflow:local/review:latest").unwrap();
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
                    content: "Workflow execution is not implemented yet.".into()
                }
            );
            return;
        }
    }
    panic!("workflow tool response was not emitted");
}
