use std::collections::BTreeSet;

use agent_plugin::{
    AgentContext, AgentCreateRequest, AgentCreated, AgentDefaultVisibility, AgentDynamicVisibility,
    AgentPlugin, AgentWorkspaceId,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;

#[test]
fn documented_public_api_creates_an_agent() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AgentPlugin::default());
    let workspace = app.world_mut().spawn();
    app.world().send_event(AgentCreateRequest {
        id: "agent-1".into(),
        agent_id: "test.agent0".into(),
        workspace_id: workspace,
        system_prompt: "You are concise.".into(),
        messages: Vec::new(),
        tool_context: Vec::new(),
        default_visibility: BTreeSet::new(),
    });
    app.tick();
    app.tick();

    let created = app
        .world()
        .event_reader::<AgentCreated>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(created.id, "agent-1");
    let agent = created.agent;
    assert_eq!(
        app.world()
            .get_component::<AgentWorkspaceId>(agent)
            .unwrap()
            .workspace_id(),
        workspace
    );
    assert_eq!(
        app.world()
            .get_component::<AgentContext>(agent)
            .unwrap()
            .system_prompt(),
        "You are concise."
    );
    assert!(app
        .world()
        .get_component::<AgentDefaultVisibility>(agent)
        .unwrap()
        .resources()
        .is_empty());
    assert!(app
        .world()
        .get_component::<AgentDynamicVisibility>(agent)
        .unwrap()
        .resources()
        .is_empty());
}
