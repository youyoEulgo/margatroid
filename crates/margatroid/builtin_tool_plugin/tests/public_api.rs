use std::fs;

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use builtin_tool_plugin::BuiltinToolPlugin;
use core_plugin::App;
use margatroid_types::ResourceId;
use tempfile::tempdir;
use tool_plugin::{
    attach_agent_tool_map, AgentToolEnvironment, AgentToolMap, AgentToolRegisterRequest,
    AgentToolRegisterResponse, ToolPlugin,
};

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
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        AgentToolEnvironment::new(project.path(), image.path()),
    );
    attach_agent_tool_map(app.world_mut(), agent).unwrap();

    let resource_id = ResourceId::parse("shell:local/bash:latest").unwrap();
    app.world().send_event(AgentToolRegisterRequest {
        id: "register-shell".into(),
        agent,
        resource_id: resource_id.clone(),
    });
    for _ in 0..4 {
        app.tick();
    }

    let response = app
        .world()
        .event_reader::<AgentToolRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-shell")
        .unwrap();
    assert!(response.result.is_ok());
    let mapping = app
        .world()
        .get_component::<AgentToolMap>(agent)
        .unwrap()
        .get_by_resource(&resource_id);
    assert_eq!(mapping.len(), 1);
    assert_eq!(
        mapping[0].tool_id,
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
    app.world().send_event(AgentToolRegisterRequest {
        id: "register-builtin".into(),
        agent,
        resource_id,
    });
    app.tick();
    app.tick();

    let response = app
        .world()
        .event_reader::<AgentToolRegisterResponse>()
        .into_iter()
        .find(|response| response.id == "register-builtin")
        .unwrap();
    assert!(response.result.is_err());
}
