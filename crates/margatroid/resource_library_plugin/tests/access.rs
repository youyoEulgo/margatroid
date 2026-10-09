use std::sync::Arc;

use core_plugin::{App, Entity, World};
use resource_id_plugin::{ResourceIdPlugin, ResourceRef};
use resource_library_plugin::{
    hook_resource, install_resource, prompt_resource, read_skill_content, read_tool_script,
    sandbox_resource, skill_resource, tool_definition, tool_resource, HookResource, PromptResource,
    ResourceDefinition, SandboxResource, SkillResource, ToolResource,
};

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(ResourceIdPlugin);
    app.world_mut()
        .insert_resource(resource_library_plugin::ResourceLibraryRoot(
            std::env::temp_dir(),
        ));
    app
}

fn owner(world: &mut World) -> Entity {
    world.spawn()
}

fn reference(value: &str) -> ResourceRef {
    ResourceRef::parse(value).unwrap()
}

fn install(world: &mut World, id: &str, definition: ResourceDefinition) -> Entity {
    let owner = owner(world);
    install_resource(world, reference(id), owner, definition).unwrap()
}

#[test]
fn every_resource_type_is_readable_through_its_accessor() {
    let mut app = app();
    let world = app.world_mut();
    let root = std::env::temp_dir().join(format!("margatroid-access-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("main.lua");
    let content = root.join("SKILL.md");
    std::fs::write(&script, "function execute() return 1 end").unwrap();
    std::fs::write(&content, "body").unwrap();

    install(
        world,
        "tool:local/probe:latest",
        ResourceDefinition::Tool(ToolResource {
            description: "Probe.".into(),
            input_schema: serde_json::json!({"type":"object"}),
            script_path: script.clone(),
        }),
    );
    let skill = install(
        world,
        "skill:local/probe:latest",
        ResourceDefinition::Skill(SkillResource {
            description: "Skill probe.".into(),
            content_path: content.clone(),
        }),
    );
    let prompt = install(
        world,
        "prompt:local/probe:latest",
        ResourceDefinition::Prompt(PromptResource {
            role: "system".into(),
            content: Arc::from("prompt body"),
        }),
    );
    let sandbox = install(
        world,
        "sandbox:local/probe:latest",
        ResourceDefinition::Sandbox(SandboxResource {
            policy: Arc::from(r#"{"rules":[]}"#),
        }),
    );
    let hook = install(
        world,
        "hook:local/probe:latest",
        ResourceDefinition::Hook(HookResource {
            description: "Hook probe.".into(),
            input_schema: serde_json::json!({"type":"object"}),
        }),
    );

    assert_eq!(
        tool_resource(world, &reference("tool:local/probe:latest"))
            .unwrap()
            .script_path,
        script
    );
    assert_eq!(
        read_tool_script(world, &reference("tool:local/probe:latest")).unwrap(),
        "function execute() return 1 end"
    );
    assert_eq!(
        skill_resource(world, &reference("skill:local/probe:latest"))
            .unwrap()
            .content_path,
        content
    );
    assert_eq!(
        read_skill_content(world, &reference("skill:local/probe:latest")).unwrap(),
        "body"
    );
    assert_eq!(
        &*prompt_resource(world, &reference("prompt:local/probe:latest"))
            .unwrap()
            .content,
        "prompt body"
    );
    assert_eq!(
        &*sandbox_resource(world, &reference("sandbox:local/probe:latest"))
            .unwrap()
            .policy,
        r#"{"rules":[]}"#
    );
    assert_eq!(
        hook_resource(world, &reference("hook:local/probe:latest"))
            .unwrap()
            .description,
        "Hook probe."
    );

    assert!(world.is_alive(skill));
    assert!(world.is_alive(prompt));
    assert!(world.is_alive(sandbox));
    assert!(world.is_alive(hook));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_typed_accessor_never_returns_a_resource_of_another_type() {
    let mut app = app();
    let world = app.world_mut();
    install(
        world,
        "prompt:local/only:latest",
        ResourceDefinition::Prompt(PromptResource {
            role: "user".into(),
            content: Arc::from("x"),
        }),
    );

    let id = reference("prompt:local/only:latest");
    assert!(tool_resource(world, &id).is_err());
    assert!(skill_resource(world, &id).is_err());
    assert!(sandbox_resource(world, &id).is_err());
    assert!(hook_resource(world, &id).is_err());
    assert!(read_tool_script(world, &id).is_err());
    assert!(read_skill_content(world, &id).is_err());
    assert!(prompt_resource(world, &id).is_ok());
}

#[test]
fn an_unreadable_file_is_reported_against_its_address() {
    let mut app = app();
    let world = app.world_mut();
    let missing = std::env::temp_dir().join("margatroid-absent-script.lua");
    let _ = std::fs::remove_file(&missing);
    install(
        world,
        "tool:local/gone:latest",
        ResourceDefinition::Tool(ToolResource {
            description: "Gone.".into(),
            input_schema: serde_json::json!({"type":"object"}),
            script_path: missing,
        }),
    );

    let id = reference("tool:local/gone:latest");
    assert!(tool_resource(world, &id).is_ok());
    let error = read_tool_script(world, &id).unwrap_err();
    assert!(
        error.to_string().contains("tool:local/gone:latest"),
        "the address must be part of the error: {error}"
    );
}

#[test]
fn an_executable_definition_covers_tool_skill_and_hook() {
    let mut app = app();
    let world = app.world_mut();
    install(
        world,
        "tool:local/probe:latest",
        ResourceDefinition::Tool(ToolResource {
            description: "Tool probe.".into(),
            input_schema: serde_json::json!({"type":"object","required":["a"]}),
            script_path: std::env::temp_dir().join("unused.lua"),
        }),
    );
    install(
        world,
        "skill:local/probe:latest",
        ResourceDefinition::Skill(SkillResource {
            description: "Skill probe.".into(),
            content_path: std::env::temp_dir().join("unused.md"),
        }),
    );
    install(
        world,
        "hook:local/probe:latest",
        ResourceDefinition::Hook(HookResource {
            description: "Hook probe.".into(),
            input_schema: serde_json::json!({"type":"object","required":["b"]}),
        }),
    );

    let tool = tool_definition(world, &reference("tool:local/probe:latest")).unwrap();
    assert_eq!(tool.description, "Tool probe.");
    assert_eq!(tool.input_schema["required"][0], serde_json::json!("a"));

    let skill = tool_definition(world, &reference("skill:local/probe:latest")).unwrap();
    assert_eq!(skill.description, "Skill probe.");
    assert_eq!(skill.input_schema, serde_json::json!({"type":"object"}));

    let hook = tool_definition(world, &reference("hook:local/probe:latest")).unwrap();
    assert_eq!(hook.description, "Hook probe.");
    assert_eq!(hook.input_schema["required"][0], serde_json::json!("b"));

    let prompt = install(
        world,
        "prompt:local/probe:latest",
        ResourceDefinition::Prompt(PromptResource {
            role: "system".into(),
            content: Arc::from("x"),
        }),
    );
    assert!(world.is_alive(prompt));
    assert!(tool_definition(world, &reference("prompt:local/probe:latest")).is_err());
}

#[test]
fn a_missing_address_is_reported_as_missing() {
    let app = app();
    let world = app.world();
    let id = reference("tool:local/never:latest");
    assert!(tool_resource(world, &id).is_err());
    assert!(read_tool_script(world, &id).is_err());
    assert!(tool_definition(world, &id).is_err());
}

#[test]
fn the_library_root_is_where_packages_are_resolved() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.add_plugin(ResourceIdPlugin);
    let world = app.world_mut();
    world.insert_resource(resource_library_plugin::ResourceLibraryRoot(
        directory.path().to_path_buf(),
    ));
    assert_eq!(
        world
            .get_resource::<resource_library_plugin::ResourceLibraryRoot>()
            .unwrap()
            .0,
        directory.path()
    );
}
