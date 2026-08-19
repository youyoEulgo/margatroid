use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::{ResourceId, ToolCall};
use serde_json::json;
use tool_plugin::{
    attach_agent_tool_map, register_agent_tool, set_agent_tool_alias, AgentToolMap, ToolCallEvent,
    ToolCallRequest, ToolPlugin, ToolTemplate,
};

#[test]
fn agent_tool_maps_route_local_tool_names() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default());
    let agent = app.world_mut().spawn();
    attach_agent_tool_map(app.world_mut(), agent).unwrap();
    let tool_id = ResourceId::parse("tool:builtin/skill-loader:latest").unwrap();
    let resource_id = ResourceId::parse("skill:local/review:latest").unwrap();
    let map = register_agent_tool(
        app.world_mut(),
        agent,
        tool_id.clone(),
        resource_id.clone(),
        ToolTemplate::new(
            "ignored",
            "Load a skill resource.",
            json!({"type":"object"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(map.tool_name, "skill0_review");

    app.world().send_event(ToolCallEvent {
        turn_id: "turn-1".into(),
        agent,
        call: ToolCall {
            id: "call-1".into(),
            tool_name: map.tool_name,
            arguments: "{}".into(),
        },
    });
    app.tick();
    app.tick();
    let request = app
        .world()
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(request.tool_id, tool_id);
    assert_eq!(request.resource_id, resource_id);
    assert_eq!(request.tool_call_id, "call-1");
    assert!(app.world().get_component::<AgentToolMap>(agent).is_some());
}

#[test]
fn aliases_replace_generated_names_for_an_agent() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default());
    let agent = app.world_mut().spawn();
    attach_agent_tool_map(app.world_mut(), agent).unwrap();
    let resource = ResourceId::parse("skill:local/review:latest").unwrap();
    let map = register_agent_tool(
        app.world_mut(),
        agent,
        ResourceId::parse("tool:builtin/skill-loader:latest").unwrap(),
        resource.clone(),
        ToolTemplate::new("ignored", "Review.", json!({"type":"object"})).unwrap(),
    )
    .unwrap();
    assert_eq!(map.tool_name, "skill0_review");
    set_agent_tool_alias(app.world_mut(), agent, resource, "review_skill".into()).unwrap();
    let map = app
        .world()
        .get_component::<AgentToolMap>(agent)
        .unwrap()
        .get_by_name("review_skill")
        .unwrap();
    assert_eq!(map.alias.as_deref(), Some("review_skill"));
    assert_eq!(map.template.name, "review_skill");
}
