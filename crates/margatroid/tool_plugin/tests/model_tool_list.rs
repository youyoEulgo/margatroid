use std::sync::Arc;

use agent_plugin::{
    Agent, AgentCreateReply, AgentCreationState, AgentInferenceState, AgentInfo, AgentLuaState,
    AgentMemoryHandle, AgentMemoryStore, AgentMemoryStoreError, AgentModelInfo, AgentToolState,
};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity, World};
use margatroid_types::ResourceRef;
use resource_id_plugin::{ResourceIdPlugin, WorldResourceIdExt};
use resource_library_plugin::{install_resource, ResourceDefinition, ToolResource};
use tool_plugin::{
    candidate_resource_entry, register_agent_resource, resolve_agent_tool_definitions, ToolPlugin,
    ToolTemplate,
};

struct EmptyMemory;

impl AgentMemoryStore for EmptyMemory {
    fn set_state(&self, _key: &str, _value: &str) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }

    fn state_value(&self, _key: &str) -> Result<Option<String>, AgentMemoryStoreError> {
        Ok(None)
    }

    fn append_record(
        &self,
        _kind: &str,
        _content: &str,
        _payload: &str,
        _source: &str,
    ) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }

    fn append_history(
        &self,
        _turn_id: &str,
        _source: &str,
        _message: &margatroid_types::Message,
        _tool_schema: &[margatroid_types::ToolDefinition],
        _usage: Option<&margatroid_types::TokenUsage>,
    ) -> Result<(), AgentMemoryStoreError> {
        Ok(())
    }

    fn history_messages(&self) -> Result<Vec<agent_plugin::HistoryMessage>, AgentMemoryStoreError> {
        Ok(Vec::new())
    }
}

fn app() -> App {
    let directory = std::env::temp_dir();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourceIdPlugin)
        .add_plugin(ToolPlugin::open(directory).unwrap());
    app
}

fn spawn_agent(world: &mut World) -> Entity {
    let workspace = world.spawn();
    let image = world.spawn();
    let entity = world.spawn();
    let (sender, _receiver) =
        tokio::sync::oneshot::channel::<Result<Entity, agent_plugin::AgentError>>();
    let model = AgentModelInfo {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        context_window_tokens: 1024,
    };
    world
        .attach_resource_id(entity, "agent:test/coder:latest".parse().unwrap())
        .unwrap();
    world.insert_component(
        entity,
        Agent {
            info: AgentInfo {
                image_entity: image,
                workspace_id: workspace,
                model: model.clone(),
                project_root: Default::default(),
                image_root: Default::default(),
                home_root: Default::default(),
                image_dependencies: Default::default(),
            },
            creation: AgentCreationState {
                request_id: "create-1".to_owned(),
                reply: AgentCreateReply::new(sender),
                initialization: Default::default(),
            },
            mcl: Default::default(),
            resources: Default::default(),
            memory: AgentMemoryHandle::new(Arc::new(EmptyMemory)),
            inference: AgentInferenceState {
                model,
                pending: Default::default(),
            },
            tools: AgentToolState::default(),
            lua: AgentLuaState::default(),
            lifecycle: agent_plugin::AgentLifecycleState::Running,
            turn: Default::default(),
            token_usage: Default::default(),
            last_error: None,
        },
    );
    entity
}

fn register(
    world: &mut World,
    agent: Entity,
    description: &str,
    schema: serde_json::Value,
) -> ResourceRef {
    let reference: ResourceRef = "tool:local/probe:latest".parse().unwrap();
    install_resource(
        world,
        reference.clone(),
        agent,
        ResourceDefinition::Tool(ToolResource {
            description: description.to_owned(),
            input_schema: schema,
            script_path: std::env::temp_dir().join("probe.lua"),
        }),
    )
    .unwrap();
    let entry = candidate_resource_entry(
        reference.clone(),
        Some("probe".to_owned()),
        agent_plugin::ToolExecutor::LuaRuntime,
        ToolTemplate::new(
            "probe",
            "A stale description that must never reach the model.",
            serde_json::json!({"type": "object"}),
        )
        .unwrap(),
    )
    .unwrap();
    register_agent_resource(world, agent, entry).unwrap();
    reference
}

#[test]
fn the_model_sees_the_description_and_schema_the_library_holds() {
    let mut app = app();
    let world = app.world_mut();
    let agent = spawn_agent(world);
    let reference = register(
        world,
        agent,
        "The library's own description.",
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    );

    let definitions = resolve_agent_tool_definitions(world, agent, &[reference]).unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(
        definitions[0].name, "probe",
        "the agent-private visible name is what the model calls"
    );
    assert_eq!(
        definitions[0].description, "The library's own description.",
        "the description must come from the resource, not a per-agent copy"
    );
    assert_eq!(
        definitions[0].input_schema["required"][0],
        serde_json::json!("path"),
        "the schema must come from the resource, not a per-agent copy"
    );
}

#[test]
fn a_visible_resource_the_library_cannot_serve_is_refused() {
    let mut app = app();
    let world = app.world_mut();
    let agent = spawn_agent(world);
    let reference: ResourceRef = "tool:local/absent:latest".parse().unwrap();

    let entry = candidate_resource_entry(
        reference.clone(),
        Some("absent".to_owned()),
        agent_plugin::ToolExecutor::LuaRuntime,
        ToolTemplate::new("absent", "Absent.", serde_json::json!({"type": "object"})).unwrap(),
    )
    .unwrap();
    register_agent_resource(world, agent, entry).unwrap();

    assert!(
        resolve_agent_tool_definitions(world, agent, &[reference]).is_err(),
        "a tool the library cannot serve must not reach the model"
    );
}
