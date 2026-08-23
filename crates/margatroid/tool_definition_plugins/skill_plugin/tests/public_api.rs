use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::App;
use margatroid_types::ResourceId;
use skill_plugin::SkillPlugin;
use std::sync::Arc;
use tempfile::tempdir;
use tool_plugin::{ToolCallRequest, ToolCallResponse, ToolPlugin};
use tool_plugin::{ToolRegisterRequest, ToolRegisterResponse};

fn attach_agent(
    app: &mut App,
    project: &std::path::Path,
    image: &std::path::Path,
) -> core_plugin::Entity {
    let world = app.world_mut();
    let entity = world.spawn();
    let workspace = world.spawn();
    let image_entity = world.spawn();
    let model = agent_plugin::AgentModelInfo {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        context_window_tokens: 1024,
    };
    let (sender, _receiver) = tokio::sync::oneshot::channel();
    world.insert_component(
        entity,
        agent_plugin::Agent {
            info: agent_plugin::AgentInfo {
                image_entity,
                workspace_id: workspace,
                model: model.clone(),
                project_root: project.to_path_buf(),
                image_root: image.to_path_buf(),
                home_root: Default::default(),
                image_dependencies: Default::default(),
                image_sources: Default::default(),
            },
            creation: agent_plugin::AgentCreationState {
                request_id: "test".to_owned(),
                reply: agent_plugin::AgentCreateReply::new(sender),
                initialization: Default::default(),
            },
            mcl: Default::default(),
            resources: Default::default(),
            memory: agent_plugin::AgentMemoryHandle::new(Arc::new(TestMemory)),
            inference: agent_plugin::AgentInferenceState {
                model,
                pending: Default::default(),
            },
            tools: Default::default(),
            lua: Default::default(),
            lifecycle: agent_plugin::AgentLifecycleState::Running,
            turn: Default::default(),
            token_usage: Default::default(),
            last_error: None,
        },
    );
    entity
}

struct TestMemory;
impl agent_plugin::AgentMemoryStore for TestMemory {
    fn append_history(
        &self,
        _turn_id: &str,
        _message: &margatroid_types::Message,
        _tool_schema: &[margatroid_types::ToolDefinition],
        _usage: Option<&margatroid_types::TokenUsage>,
    ) -> Result<(), agent_plugin::AgentMemoryStoreError> {
        Ok(())
    }
    fn rewrite_realtime(
        &self,
        _messages: &[margatroid_types::MclMessage],
    ) -> Result<(), agent_plugin::AgentMemoryStoreError> {
        Ok(())
    }
    fn read_realtime(
        &self,
    ) -> Result<Vec<margatroid_types::MclMessage>, agent_plugin::AgentMemoryStoreError> {
        Ok(Vec::new())
    }
    fn history_messages(
        &self,
    ) -> Result<Vec<agent_plugin::HistoryMessage>, agent_plugin::AgentMemoryStoreError> {
        Ok(Vec::new())
    }
}

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
    let agent = attach_agent(&mut app, project.path(), image.path());
    let resource = ResourceId::parse("skill:local/review:latest").unwrap();
    app.world().send_event(ToolRegisterRequest {
        id: "register-1".into(),
        agent,
        resource_id: resource.clone(),
        alias: None,
    });
    app.tick();
    app.tick();
    let registration = app
        .world()
        .event_reader::<ToolRegisterResponse>()
        .into_iter()
        .next()
        .unwrap();
    assert!(registration.result.is_ok());
    let mapping = registration.result.as_ref().unwrap();
    assert_eq!(mapping.resource_id, resource);
    assert_eq!(
        mapping.template.as_ref().unwrap().description,
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
