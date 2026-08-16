use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

use agent_plugin::{AgentCreateRequest, AgentCreateResult, AgentPlugin};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use lua_plugin::{LuaPlugin, LuaToolRegisterRequest, LuaToolRegisterResponse};
use margatroid_types::ResourceId;
use tempfile::tempdir;
use tool_plugin::{
    AgentToolEnvironment, AgentToolMap, ToolCallRequest, ToolCallResponse, ToolPlugin,
};

#[test]
fn registers_and_executes_trusted_lua_tools_asynchronously() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let home = tempdir().unwrap();
    let package = home.path().join("local/echo/latest");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("tool.toml"),
        "schema_version = 1\nname = \"echo\"\ndescription = \"Echo structured input.\"\n",
    )
    .unwrap();
    fs::write(
        package.join("input.schema.json"),
        r#"{
            "type": "object",
            "properties": {
                "value": { "type": "string" },
                "output": { "type": "string" }
            },
            "required": ["value", "output"],
            "additionalProperties": false
        }"#,
    )
    .unwrap();
    fs::write(package.join("main.lua"), "this is not valid Lua").unwrap();

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(LuaPlugin::open(home.path()).unwrap());
    let workspace = app.world_mut().spawn();
    app.world().send_event(AgentCreateRequest {
        id: "create-1".into(),
        agent_id: ResourceId::parse("agent:demo/coder:latest").unwrap(),
        workspace_id: workspace,
        system_prompt: "test".into(),
        messages: Vec::new(),
        tool_context: Vec::new(),
        default_visibility: BTreeSet::new(),
    });
    app.tick();
    app.tick();
    let agent = app
        .world()
        .event_reader::<AgentCreateResult>()
        .into_iter()
        .find(|event| event.id == "create-1")
        .unwrap()
        .result
        .as_ref()
        .copied()
        .unwrap();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );

    let resource_id = ResourceId::parse("tool:local/echo:latest").unwrap();
    app.world().send_event(LuaToolRegisterRequest {
        id: "register-1".into(),
        agent,
        resource_id: resource_id.clone(),
    });
    app.tick();
    app.tick();
    let registration = app
        .world()
        .event_reader::<LuaToolRegisterResponse>()
        .into_iter()
        .find(|event| event.id == "register-1")
        .unwrap();
    assert!(registration.result.is_ok());
    let maps = app.world().get_component::<AgentToolMap>(agent).unwrap();
    let mapping = maps.get_by_resource(&resource_id);
    assert_eq!(mapping.len(), 1);
    assert_eq!(mapping[0].template.description, "Echo structured input.");

    fs::write(
        package.join("main.lua"),
        r#"
function execute(arguments, context)
    local encoded = margatroid.json.encode({
        value = arguments.value,
        agent = context.agent_id,
        project = context.project_root,
        has_path = os.getenv("PATH") ~= nil
    })
    margatroid.fs.write_text(arguments.output, encoded)
    margatroid.log.info("test Lua tool executed")
    return margatroid.fs.read_text(arguments.output)
end
"#,
    )
    .unwrap();
    let output = project.path().join("result.json");
    app.world().send_event(ToolCallRequest {
        turn_id: "turn-1".into(),
        agent,
        tool_id: ResourceId::parse("tool:builtin/lua-runtime:latest").unwrap(),
        resource_id,
        tool_call_id: "call-1".into(),
        arguments: serde_json::json!({
            "value": "hello",
            "output": output.to_string_lossy()
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
            let result = response.result.as_ref().unwrap();
            let result = serde_json::from_str::<serde_json::Value>(result).unwrap();
            assert_eq!(result["value"], "hello");
            assert_eq!(result["agent"], "agent:demo/coder:latest");
            assert_eq!(result["project"], project.path().to_string_lossy().as_ref());
            assert_eq!(result["has_path"], true);
            assert_eq!(
                fs::read_to_string(output).unwrap(),
                response.result.as_ref().unwrap().as_str()
            );
            return;
        }
        assert!(Instant::now() < deadline, "Lua tool response timed out");
        std::thread::yield_now();
    }
}

#[test]
fn tracked_write_file_example_creates_parent_directories_and_writes_content() {
    let project = tempdir().unwrap();
    let image = tempdir().unwrap();
    let examples = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/tools");

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(LuaPlugin::open(examples).unwrap());
    let workspace = app.world_mut().spawn();
    app.world().send_event(AgentCreateRequest {
        id: "create-write-agent".into(),
        agent_id: ResourceId::parse("agent:demo/writer:latest").unwrap(),
        workspace_id: workspace,
        system_prompt: "test".into(),
        messages: Vec::new(),
        tool_context: Vec::new(),
        default_visibility: BTreeSet::new(),
    });
    app.tick();
    app.tick();
    let agent = app
        .world()
        .event_reader::<AgentCreateResult>()
        .into_iter()
        .find(|event| event.id == "create-write-agent")
        .unwrap()
        .result
        .as_ref()
        .copied()
        .unwrap();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );

    let resource_id = ResourceId::parse("tool:local/write-file:latest").unwrap();
    app.world().send_event(LuaToolRegisterRequest {
        id: "register-write-file".into(),
        agent,
        resource_id: resource_id.clone(),
    });
    app.tick();
    app.tick();
    let registration = app
        .world()
        .event_reader::<LuaToolRegisterResponse>()
        .into_iter()
        .find(|event| event.id == "register-write-file")
        .unwrap();
    assert!(registration.result.is_ok());

    app.world().send_event(ToolCallRequest {
        turn_id: "write-turn".into(),
        agent,
        tool_id: ResourceId::parse("tool:builtin/lua-runtime:latest").unwrap(),
        resource_id,
        tool_call_id: "write-call".into(),
        arguments: serde_json::json!({
            "path": "nested/result.txt",
            "content": "complete contents",
            "create_parent_directories": true
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
            .find(|response| response.tool_call_id == "write-call")
        {
            let result = serde_json::from_str::<serde_json::Value>(
                response.result.as_ref().expect("write-file must succeed"),
            )
            .unwrap();
            assert_eq!(result["bytes"], 17);
            assert_eq!(result["replaced"], true);
            assert_eq!(
                fs::read_to_string(project.path().join("nested/result.txt")).unwrap(),
                "complete contents"
            );
            return;
        }
        assert!(Instant::now() < deadline, "write-file response timed out");
        std::thread::yield_now();
    }
}
