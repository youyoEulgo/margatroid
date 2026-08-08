use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::{ResourceName, ResourceRef};
use tempfile::tempdir;
use tool_plugin::{AgentToolEnvironment, ToolPlugin, WorldToolExt};
use workflow_plugin::WorkflowPlugin;

#[test]
fn documented_public_api_registers_the_workflow_provider() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("local/review")).unwrap();

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default())
        .add_plugin(WorkflowPlugin::open(home.path()).unwrap());
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );
    let resource =
        ResourceRef::new("workflow", ResourceName::new("local/review").unwrap()).unwrap();

    let tool = app.world().resolve_tool(agent, &resource).unwrap();

    assert_eq!(tool.resource(), &resource);
    assert_eq!(tool.definition().name, "workflow_local_review");
}
