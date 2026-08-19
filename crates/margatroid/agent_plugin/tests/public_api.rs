use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use agent_plugin::{
    AgentContext, AgentContextCompactRequest, AgentCreateRequest, AgentCreateResult, AgentCreated,
    AgentPlugin, AgentTokenUsage, AgentWorkspaceId, WorldAgentExt,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::{ResourceId, TokenUsage};
use mcl_plugin::{compile_mcl, AgentMcl, MclCompileRequest, MclPlugin, MclSource};
use tool_plugin::ToolPlugin;

fn base_mcl() -> std::sync::Arc<mcl_plugin::MclProgram> {
    compile_mcl(MclCompileRequest {
        root: MclSource::new(
            ResourceId::parse("mcl:local/test:latest").unwrap(),
            r#"base context test {
block conversation: context persistent;
view messages: messages { select entry from conversation; }
view tools: tools { select resource from capabilities.dynamic; }
request inference { system = agent.system; messages = messages; tools = tools; }
on agent.created { restore capabilities.dynamic from capabilities.default; }
}"#,
            PathBuf::from("/test/main.mcl"),
        ),
        dependencies: BTreeMap::new(),
    })
    .unwrap()
}

#[test]
fn documented_public_api_creates_an_agent() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(MclPlugin::open(std::env::temp_dir()).unwrap())
        .add_plugin(AgentPlugin::default());
    let workspace = app.world_mut().spawn();
    app.world().send_event(AgentCreateRequest {
        id: "agent-1".into(),
        agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
        workspace_id: workspace,
        base_mcl: base_mcl(),
        system_prompt: "You are concise.".into(),
        messages: Vec::new(),
        tool_context: Vec::new(),
        ordered_messages: Vec::new(),
        token_usage: TokenUsage {
            input_tokens: 200,
            output_tokens: 40,
            cache_hit_tokens: 150,
        },
        last_input_tokens: 200,
        context_window_tokens: 1_000_000,
        default_visibility: BTreeSet::new(),
    });
    app.tick();
    app.tick();

    let created = app
        .world()
        .event_reader::<AgentCreateResult>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(created.id, "agent-1");
    let agent = created.result.as_ref().copied().unwrap();
    assert!(app
        .world()
        .event_reader::<AgentCreated>()
        .into_iter()
        .any(|event| event.id == "agent-1" && event.agent == agent));
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
        .get_component::<AgentMcl>(agent)
        .unwrap()
        .capabilities()
        .default_resources()
        .is_empty());
    let usage = app.world().get_component::<AgentTokenUsage>(agent).unwrap();
    assert_eq!(usage.total_input_tokens(), 200);
    assert_eq!(usage.total_output_tokens(), 40);
    assert_eq!(usage.total_cache_hit_tokens(), 150);
    assert_eq!(usage.cache_hit_rate(), 0.75);
    assert!(app
        .world()
        .get_component::<AgentMcl>(agent)
        .unwrap()
        .capabilities()
        .visible_resources()
        .next()
        .is_none());
}

#[test]
fn duplicate_agent_resource_ids_are_rejected() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolPlugin::default())
        .add_plugin(MclPlugin::open(std::env::temp_dir()).unwrap())
        .add_plugin(AgentPlugin::default());
    let workspace = app.world_mut().spawn();
    let agent_id = ResourceId::parse("agent:test/agent0").unwrap();

    for request_id in ["agent-1", "agent-2"] {
        app.world().send_event(AgentCreateRequest {
            id: request_id.into(),
            agent_id: agent_id.clone(),
            workspace_id: workspace,
            base_mcl: base_mcl(),
            system_prompt: String::new(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            ordered_messages: Vec::new(),
            token_usage: margatroid_types::TokenUsage::default(),
            last_input_tokens: 0,
            context_window_tokens: 1_000_000,
            default_visibility: BTreeSet::new(),
        });
    }
    app.tick();
    app.tick();

    let created = app
        .world()
        .event_reader::<AgentCreateResult>()
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 2);
    let successful = created
        .iter()
        .filter_map(|result| result.result.as_ref().ok().copied())
        .collect::<Vec<_>>();
    assert_eq!(successful.len(), 1);
    assert_eq!(app.world().agent(&agent_id), Some(successful[0]));
}

#[test]
fn context_compaction_request_is_public() {
    fn assert_event<EventType: core_plugin::Event>() {}
    assert_event::<AgentContextCompactRequest>();

    let mut app = App::new();
    let agent = app.world_mut().spawn();
    let request = AgentContextCompactRequest {
        id: "compact-1".into(),
        agent,
        retain_messages: 4,
    };
    assert_eq!(request.agent, agent);
    assert_eq!(request.retain_messages, 4);
}
