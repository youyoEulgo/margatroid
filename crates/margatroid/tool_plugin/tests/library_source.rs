use std::path::Path;
use std::sync::Arc;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use margatroid_types::ResourceRef;
use resource_id_plugin::{ResourceIdPlugin, WorldResourceIdExt};
use resource_library_plugin::{
    install_resource, read_tool_script, tool_definition, tool_resource, ResourceDefinition,
    ToolResource,
};
use tool_plugin::{run_lua_tool, LuaExecutionLimits, LuaToolRunRequest, ToolPlugin};

fn app_with_library(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourceIdPlugin)
        .add_plugin(ToolPlugin::open(root).unwrap());
    app
}

fn limits() -> LuaExecutionLimits {
    LuaExecutionLimits::new(
        1 << 20,
        1 << 20,
        1 << 20,
        1 << 20,
        1 << 28,
        1_000_000_000,
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(30),
    )
    .unwrap()
}

/// Write a tool package the way the library expects to find one.
fn write_package(root: &Path, name: &str, description: &str, schema: &str, script: &str) {
    let package = root.join("tools/local").join(name).join("latest");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("tool.toml"),
        format!("schema_version = 1\nname = \"{name}\"\ndescription = \"{description}\"\n"),
    )
    .unwrap();
    std::fs::write(package.join("input.schema.json"), schema).unwrap();
    std::fs::write(package.join("main.lua"), script).unwrap();
}

const ECHO_SCRIPT: &str = r#"
function execute(arguments, context)
    return "echo:" .. tostring(arguments.text)
end
"#;

fn install_echo_tool(world: &mut core_plugin::World, root: &Path) -> Entity {
    write_package(
        root,
        "echo",
        "Echo the text back.",
        r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#,
        ECHO_SCRIPT,
    );
    let owner = world.spawn();
    install_resource(
        world,
        ResourceRef::parse("tool:local/echo:latest").unwrap(),
        owner,
        ResourceDefinition::Tool(ToolResource {
            description: "Echo the text back.".to_owned(),
            input_schema: serde_json::json!({
                "type":"object",
                "properties":{"text":{"type":"string"}},
                "required":["text"]
            }),
            script_path: root.join("tools/local/echo/latest/main.lua").to_path_buf(),
        }),
    )
    .unwrap()
}

#[test]
fn the_library_is_the_only_source_of_a_tool_definition() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_with_library(directory.path());
    let world = app.world_mut();
    let entity = install_echo_tool(world, directory.path());

    let reference = ResourceRef::parse("tool:local/echo:latest").unwrap();
    assert_eq!(
        world.entity_by_resource_id(&reference).unwrap(),
        entity,
        "the library resolves the tool address it installed"
    );

    let definition = tool_definition(world, &reference).unwrap();
    assert_eq!(definition.description, "Echo the text back.");
    assert_eq!(
        definition.input_schema["properties"]["text"]["type"],
        serde_json::json!("string")
    );
    assert_eq!(
        definition.input_schema["required"][0],
        serde_json::json!("text")
    );

    let script = read_tool_script(world, &reference).unwrap();
    assert!(script.contains("echo:"));
    assert_eq!(
        tool_resource(world, &reference).unwrap().script_path,
        directory.path().join("tools/local/echo/latest/main.lua")
    );
}

#[test]
fn a_tool_runs_from_the_component_without_reparsing_its_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_with_library(directory.path());
    let world = app.world_mut();
    install_echo_tool(world, directory.path());

    let reference = ResourceRef::parse("tool:local/echo:latest").unwrap();
    let definition = tool_definition(world, &reference).unwrap();
    let script = read_tool_script(world, &reference).unwrap();

    let request = LuaToolRunRequest {
        description: definition.description,
        input_schema: definition.input_schema,
        script,
        temp_dir: directory.path().to_path_buf(),
        arguments: r#"{"text":"hello"}"#.to_owned(),
        agent_id: ResourceRef::parse("agent:local/coder:latest").unwrap(),
        turn_id: "turn-1".to_owned(),
        resource_id: reference,
        project_root: directory.path().to_path_buf(),
        limits: limits(),
        sandbox_policies: Vec::new(),
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let output = runtime.block_on(run_lua_tool(request)).unwrap();
    assert_eq!(output, "echo:hello");
}

#[test]
fn a_description_that_is_not_toml_still_runs() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_with_library(directory.path());
    let world = app.world_mut();
    let awkward = "Use this, e.g. when: nothing = else applies.";
    write_package(
        directory.path(),
        "echo",
        awkward,
        r#"{"type":"object"}"#,
        ECHO_SCRIPT,
    );
    let owner = world.spawn();
    let reference = ResourceRef::parse("tool:local/echo:latest").unwrap();
    install_resource(
        world,
        reference.clone(),
        owner,
        ResourceDefinition::Tool(ToolResource {
            description: awkward.to_owned(),
            input_schema: serde_json::json!({"type":"object"}),
            script_path: directory
                .path()
                .join("tools/local/echo/latest/main.lua")
                .to_path_buf(),
        }),
    )
    .unwrap();

    let definition = tool_definition(world, &reference).unwrap();
    assert_eq!(definition.description, awkward);

    let request = LuaToolRunRequest {
        description: definition.description,
        input_schema: definition.input_schema,
        script: read_tool_script(world, &reference).unwrap(),
        temp_dir: directory.path().to_path_buf(),
        arguments: r#"{"text":"x"}"#.to_owned(),
        agent_id: ResourceRef::parse("agent:local/coder:latest").unwrap(),
        turn_id: "turn-1".to_owned(),
        resource_id: reference,
        project_root: directory.path().to_path_buf(),
        limits: limits(),
        sandbox_policies: Vec::new(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(runtime.block_on(run_lua_tool(request)).unwrap(), "echo:x");
}

#[test]
fn library_accessors_report_the_address_they_could_not_serve() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_with_library(directory.path());
    let world = app.world_mut();
    let missing = ResourceRef::parse("tool:local/absent:latest").unwrap();

    assert!(tool_resource(world, &missing).is_err());
    assert!(read_tool_script(world, &missing).is_err());
    assert!(tool_definition(world, &missing).is_err());
}

#[test]
fn a_resource_of_another_type_is_refused_by_a_typed_accessor() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_with_library(directory.path());
    let world = app.world_mut();
    let owner = world.spawn();
    let reference = ResourceRef::parse("prompt:local/soul:latest").unwrap();
    install_resource(
        world,
        reference.clone(),
        owner,
        ResourceDefinition::Prompt(resource_library_plugin::PromptResource {
            role: "system".to_owned(),
            content: Arc::from("hello"),
        }),
    )
    .unwrap();

    assert!(tool_resource(world, &reference).is_err());
    assert!(read_tool_script(world, &reference).is_err());
}
