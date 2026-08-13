use std::collections::BTreeSet;

use agent_plugin::{
    AgentContext, AgentCreateRequest, AgentCreated, AgentDefaultVisibility, AgentDynamicVisibility,
    AgentPlugin, AgentWorkspaceId, LoadAgentSkill, UnloadAgentSkill, UnloadAllAgentSkills,
    WorldAgentExt,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::ResourceId;

#[test]
fn documented_public_api_creates_an_agent() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AgentPlugin::default());
    let workspace = app.world_mut().spawn();
    app.world().send_event(AgentCreateRequest {
        id: "agent-1".into(),
        agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
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

#[test]
fn duplicate_agent_resource_ids_are_rejected() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AgentPlugin::default());
    let workspace = app.world_mut().spawn();
    let agent_id = ResourceId::parse("agent:test/agent0").unwrap();

    for request_id in ["agent-1", "agent-2"] {
        app.world().send_event(AgentCreateRequest {
            id: request_id.into(),
            agent_id: agent_id.clone(),
            workspace_id: workspace,
            system_prompt: String::new(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            default_visibility: BTreeSet::new(),
        });
    }
    app.tick();
    app.tick();

    let created = app
        .world()
        .event_reader::<AgentCreated>()
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 1);
    assert_eq!(app.world().agent(&agent_id), Some(created[0].agent));
}

#[test]
fn loading_skill_events_are_public() {
    fn assert_event<EventType: core_plugin::Event>() {}
    assert_event::<LoadAgentSkill>();
    assert_event::<UnloadAgentSkill>();
    assert_event::<UnloadAllAgentSkills>();

    let mut app = App::new();
    let agent = app.world_mut().spawn();
    let resource_id = ResourceId::parse("skill:local/review:latest").unwrap();
    let load = LoadAgentSkill {
        id: "load-1".into(),
        agent,
        resource_id: resource_id.clone(),
    };
    let unload = UnloadAgentSkill {
        id: "unload-1".into(),
        agent,
        resource_id,
    };
    let unload_all = UnloadAllAgentSkills {
        id: "unload-all-1".into(),
        agent,
    };
    assert_eq!(load.agent, unload.agent);
    assert_eq!(load.agent, unload_all.agent);
}
