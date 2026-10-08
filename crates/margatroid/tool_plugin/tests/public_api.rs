use agent_plugin::ToolExecutor;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::ResourceRef;
use serde_json::json;
use tool_plugin::{
    candidate_resource_entry, ToolError, ToolPlugin, ToolRegisterResponse, ToolTemplate,
};

#[test]
fn resource_registration_builds_an_aliasable_candidate() {
    let resource = ResourceRef::parse("skill:local/review:latest").unwrap();
    let entry = candidate_resource_entry(
        resource.clone(),
        Some("review_skill".into()),
        ToolExecutor::SkillLoader,
        ToolTemplate::new("ignored", "Review.", json!({"type":"object"})).unwrap(),
    )
    .unwrap();
    assert_eq!(entry.resource_id, resource);
    assert_eq!(entry.resource_name, "review_skill");
    assert_eq!(entry.alias.as_deref(), Some("review_skill"));
    assert_eq!(entry.executor, Some(ToolExecutor::SkillLoader));
}

#[test]
fn registration_response_is_an_explicit_provider_result() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default());
    let agent = app.world_mut().spawn();
    let resource = ResourceRef::parse("skill:local/review:latest").unwrap();
    app.world().send_event(ToolRegisterResponse {
        id: "registration".into(),
        agent,
        resource_id: resource.clone(),
        alias: Some("review_skill".into()),
        result: Err(ToolError::new(
            tool_plugin::ToolErrorKind::ProviderMissing,
            "test provider",
        )),
    });
    app.tick();
    let response = app
        .world()
        .event_reader::<ToolRegisterResponse>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(response.resource_id, resource);
    assert!(response.result.is_err());
}
