use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::{ResourceId, ToolCall, ToolDefinition};
use serde_json::json;
use tool_plugin::{
    AppToolExt, ToolCallEvent, ToolCallRequest, ToolPlugin, ToolTemplate, WorldToolExt,
};

#[test]
fn templates_route_domain_tool_calls() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default());
    let loader = ResourceId::parse("tool:builtin/skill-loader:latest").unwrap();
    app.register_tool_template(
        ToolTemplate::new(
            loader.clone(),
            ToolDefinition {
                name: "skill_loader".into(),
                description: "Load a skill resource.".into(),
                input_schema: json!({"type":"object"}),
            },
        )
        .unwrap(),
    );
    assert_eq!(
        app.world()
            .tool_template(&loader)
            .unwrap()
            .definition()
            .name,
        "skill_loader"
    );

    let agent = app.world_mut().spawn();
    let resource = ResourceId::parse("skill:local/review:latest").unwrap();
    app.world().send_event(ToolCallRequest {
        id: "turn-1".into(),
        agent,
        call: ToolCall {
            id: "call-1".into(),
            resource: resource.clone(),
            arguments: "{}".into(),
        },
    });
    app.tick();
    app.tick();
    let routed = app
        .world()
        .event_reader::<ToolCallEvent>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(routed.loader, loader);
    assert_eq!(routed.resource, resource);
    assert_eq!(routed.call.resource, routed.resource);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&routed.call.arguments).unwrap(),
        json!({"resource":"skill:local/review:latest"})
    );
}
