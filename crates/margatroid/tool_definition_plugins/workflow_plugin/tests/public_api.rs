use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::ResourceId;
use tempfile::tempdir;
use tool_plugin::{ToolCallRequest, ToolCallResponse, ToolPlugin};
use workflow_plugin::WorkflowPlugin;

#[test]
fn workflow_loader_returns_placeholder_tool_message() {
    let home = tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("local/review/latest")).unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(WorkflowPlugin::open(home.path()).unwrap());
    let agent = app.world_mut().spawn();
    let resource = ResourceId::parse("workflow:local/review:latest").unwrap();
    app.world().send_event(ToolCallRequest {
        turn_id: "turn-1".into(),
        agent,
        tool_id: ResourceId::parse("tool:builtin/workflow-loader:latest").unwrap(),
        resource_id: resource,
        tool_call_id: "call-1".into(),
        arguments: "{}".into(),
    });
    for _ in 0..4 {
        app.tick();
        if let Some(response) = app
            .world()
            .event_reader::<ToolCallResponse>()
            .into_iter()
            .next()
        {
            assert!(response.result.is_err());
            return;
        }
    }
    panic!("workflow tool response was not emitted");
}
