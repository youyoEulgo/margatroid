use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::ResourceId;
use shell_plugin::{ShellPlugin, ShellRegisterRequest, ShellRegisterResponse};
use tempfile::tempdir;
use tool_plugin::{ToolCallRequest, ToolCallResponse, ToolPlugin};

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
fn shell_resources_use_a_hidden_executor_and_return_process_output() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    let package = home.path().join("local/bash/latest");
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
        .add_plugin(ShellPlugin::open(home.path()).unwrap());
    let agent = attach_agent(&mut app, project.path(), image.path());

    let resource_id = ResourceId::parse("shell:local/bash:latest").unwrap();
    app.world().send_event(ShellRegisterRequest {
        id: "register-1".into(),
        agent,
        resource_id: resource_id.clone(),
        alias: None,
    });
    app.tick();
    app.tick();
    let registration = app
        .world()
        .event_reader::<ShellRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-1")
        .unwrap();
    assert!(registration.result.is_ok());
    let mapping = registration.result.as_ref().unwrap();
    assert_eq!(
        mapping.tool_id.as_ref().unwrap().clone(),
        ResourceId::parse("tool:builtin/shell:latest").unwrap()
    );

    app.world().send_event(ToolCallRequest {
        turn_id: "turn-1".into(),
        agent,
        tool_id: mapping.tool_id.clone().unwrap(),
        resource_id,
        tool_call_id: "call-1".into(),
        arguments: serde_json::json!({
            "command": "printf 'hello'; printf 'problem' >&2; exit 7"
        })
        .to_string(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if let Some(response) = app
            .world()
            .event_reader::<ToolCallResponse>()
            .into_iter()
            .find(|response| response.tool_call_id == "call-1")
        {
            let output = serde_json::from_str::<serde_json::Value>(
                response.result.as_ref().expect("Shell call must complete"),
            )
            .unwrap();
            assert_eq!(output["exit_code"], 7);
            assert_eq!(output["stdout"], "hello");
            assert_eq!(output["stderr"], "problem");
            assert_eq!(output["stdout_truncated"], false);
            assert_eq!(output["stderr_truncated"], false);
            return;
        }
        assert!(Instant::now() < deadline, "Shell response timed out");
        std::thread::yield_now();
    }
}

#[test]
fn persistent_shell_preserves_directory_and_environment_between_calls() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    let package = home.path().join("local/bash/latest");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("shell.toml"),
        "schema_version = 1\nname = \"bash\"\ndescription = \"Persistent Bash.\"\npersistent = true\n",
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
        .add_plugin(ShellPlugin::open(home.path()).unwrap());
    let agent = attach_agent(&mut app, project.path(), image.path());
    let resource_id = ResourceId::parse("shell:local/bash:latest").unwrap();
    app.world().send_event(ShellRegisterRequest {
        id: "register-persistent".into(),
        agent,
        resource_id: resource_id.clone(),
        alias: None,
    });
    app.tick();
    app.tick();
    let mapping = app
        .world()
        .event_reader::<ShellRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-persistent")
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .clone();

    for (call_id, command) in [
        ("persistent-1", "cd ..; export MARGATROID_PERSIST=ok"),
        (
            "persistent-2",
            "printf '%s:%s' \"$PWD\" \"$MARGATROID_PERSIST\"",
        ),
    ] {
        app.world().send_event(ToolCallRequest {
            turn_id: "persistent-turn".into(),
            agent,
            tool_id: mapping.tool_id.clone().unwrap(),
            resource_id: resource_id.clone(),
            tool_call_id: call_id.into(),
            arguments: serde_json::json!({ "command": command }).to_string(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.tick();
            if let Some(response) = app
                .world()
                .event_reader::<ToolCallResponse>()
                .into_iter()
                .find(|response| response.tool_call_id == call_id)
            {
                let result = response
                    .result
                    .as_ref()
                    .expect("persistent call must complete");
                if call_id == "persistent-2" {
                    let output = serde_json::from_str::<serde_json::Value>(result).unwrap();
                    assert!(
                        output["stdout"].as_str().unwrap().ends_with(":ok"),
                        "unexpected PTY output: {output:?}"
                    );
                }
                break;
            }
            assert!(
                Instant::now() < deadline,
                "persistent shell response timed out"
            );
            std::thread::yield_now();
        }
    }
}
