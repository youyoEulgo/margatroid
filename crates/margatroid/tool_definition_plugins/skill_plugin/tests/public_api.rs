use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::ResourceId;
use skill_plugin::{SkillPlugin, SkillRegisterRequest, SkillRegisterResponse};
use tempfile::tempdir;
use tool_plugin::{
    attach_agent_tool_map, AgentToolEnvironment, AgentToolMap, ToolCallRequest, ToolCallResponse,
    ToolPlugin,
};

#[test]
fn skill_loader_reads_skill_and_returns_tool_message() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    let path = home.path().join("local/review/latest/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        "+++\nname = \"review\"\ndescription = \"Review a requested change.\"\n+++\n\nReview the requested change.\n",
    )
    .unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(SkillPlugin::open(home.path()).unwrap());
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );
    attach_agent_tool_map(app.world_mut(), agent).unwrap();
    let resource = ResourceId::parse("skill:local/review:latest").unwrap();
    app.world().send_event(SkillRegisterRequest {
        id: "register-1".into(),
        agent,
        resource_id: resource.clone(),
    });
    app.tick();
    app.tick();
    let registration = app
        .world()
        .event_reader::<SkillRegisterResponse>()
        .into_iter()
        .next()
        .unwrap();
    assert!(registration.result.is_ok());
    let maps = app.world().get_component::<AgentToolMap>(agent).unwrap();
    let mapping = maps.get_by_resource(&resource);
    assert_eq!(mapping.len(), 1);
    assert_eq!(
        mapping[0].template.description,
        "Review a requested change."
    );

    app.world().send_event(ToolCallRequest {
        turn_id: "turn-1".into(),
        agent,
        tool_id: ResourceId::parse("tool:builtin/skill-loader:latest").unwrap(),
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
            assert_eq!(
                response.result.as_ref().unwrap(),
                "Review the requested change."
            );
            return;
        }
    }
    panic!("skill tool response was not emitted");
}
