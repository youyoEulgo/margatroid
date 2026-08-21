use std::fs;
use std::sync::Arc;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use builtin_tool_plugin::BuiltinToolPlugin;
use core_plugin::App;
use margatroid_types::ResourceId;
use tempfile::tempdir;
use tool_plugin::{AgentResourceRegisterRequest, AgentResourceRegisterResponse, ToolPlugin};

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
fn routes_visible_shell_resources_to_the_hidden_shell_executor() {
    let data_root = tempdir().unwrap();
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let package = data_root.path().join("shells/local/bash/latest");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("shell.toml"),
        "schema_version = 1\nname = \"bash\"\ndescription = \"Execute a Bash command.\"\n",
    )
    .unwrap();
    fs::write(
        package.join("input.schema.json"),
        r#"{
            "type": "object",
            "properties": { "command": { "type": "string", "minLength": 1 } },
            "required": ["command"],
            "additionalProperties": false
        }"#,
    )
    .unwrap();
    fs::write(package.join("main.sh"), "exec bash -lc \"$1\"\n").unwrap();

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default())
        .add_plugin(BuiltinToolPlugin::open(data_root.path()).unwrap());
    let agent = attach_agent(&mut app, project.path(), image.path());

    let resource_id = ResourceId::parse("shell:local/bash:latest").unwrap();
    app.world().send_event(AgentResourceRegisterRequest {
        id: "register-shell".into(),
        agent,
        resource_id: resource_id.clone(),
        alias: None,
    });
    for _ in 0..4 {
        app.tick();
    }

    let response = app
        .world()
        .event_reader::<AgentResourceRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-shell")
        .unwrap();
    assert!(response.result.is_ok());
    let mapping = response.result.as_ref().unwrap();
    assert_eq!(
        mapping.tool_id.as_ref().unwrap().clone(),
        ResourceId::parse("tool:builtin/shell:latest").unwrap()
    );
}

#[test]
fn rejects_builtin_executors_as_visible_resources() {
    let data_root = tempdir().unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default())
        .add_plugin(BuiltinToolPlugin::open(data_root.path()).unwrap());
    let agent = app.world_mut().spawn();
    let resource_id = ResourceId::parse("tool:builtin/shell:latest").unwrap();
    app.world().send_event(AgentResourceRegisterRequest {
        id: "register-builtin".into(),
        agent,
        resource_id,
        alias: None,
    });
    app.tick();
    app.tick();

    let response = app
        .world()
        .event_reader::<AgentResourceRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-builtin")
        .unwrap();
    assert!(response.result.is_err());
}
