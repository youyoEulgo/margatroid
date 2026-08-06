use std::convert::Infallible;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::{ResourceName, ResourceRef, ToolDefinition};
use serde::Deserialize;
use serde_json::json;
use tool_plugin::{
    AgentToolEnvironment, AppToolExt, Tool, ToolCallRequest, ToolContext, ToolPlugin, WorldToolExt,
};
use tool_plugin::{ToolDefinitionProvider, ToolError};

#[derive(Deserialize)]
struct EchoArguments {
    text: String,
}

struct SkillProvider;

impl ToolDefinitionProvider for SkillProvider {
    fn id(&self) -> &str {
        "skill"
    }

    fn provide(
        &self,
        _environment: &AgentToolEnvironment,
        name: &ResourceName,
    ) -> Result<Tool, ToolError> {
        Tool::new(
            ResourceRef::new("skill", name.clone()).unwrap(),
            ToolDefinition {
                name: "skill_review".into(),
                description: "Run the selected skill".into(),
                input_schema: json!({ "type": "object" }),
            },
            |_context: ToolContext, _arguments: serde_json::Value| async move {
                Ok::<_, Infallible>("done".into())
            },
        )
    }
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::new());
    app.register_tool(
        Tool::new(
            ResourceRef::new("tool", ResourceName::new("builtin/echo").unwrap()).unwrap(),
            ToolDefinition {
                name: "echo".into(),
                description: "Return the supplied text".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }),
            },
            |context: ToolContext, arguments: EchoArguments| async move {
                Ok::<_, Infallible>(format!(
                    "{}:{}:{}",
                    context.request_id(),
                    context.tool_call_id(),
                    arguments.text
                ))
            },
        )
        .unwrap(),
    );

    let agent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(agent, AgentToolEnvironment::new("/project", "/image"));
    let tool = app
        .world()
        .resolve_tool(
            agent,
            &ResourceRef::new("tool", ResourceName::new("builtin/echo").unwrap()).unwrap(),
        )
        .unwrap();
    assert_eq!(tool.definition().name, "echo");

    app.world().emit_event(ToolCallRequest {
        id: "request".into(),
        agent,
        resource: ResourceRef::new("tool", ResourceName::new("builtin/echo").unwrap()).unwrap(),
        call: margatroid_types::ToolCall {
            id: "call".into(),
            name: "echo".into(),
            arguments: r#"{"text":"hello"}"#.into(),
        },
    });
}

#[test]
fn external_definition_providers_resolve_resources() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::new())
        .register_tool_provider(SkillProvider);
    let agent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(agent, AgentToolEnvironment::new("/project", "/image"));

    let resource = ResourceRef::new("skill", ResourceName::new("local/review").unwrap()).unwrap();
    let tool = app.world().resolve_tool(agent, &resource).unwrap();

    assert_eq!(tool.resource(), &resource);
    assert_eq!(tool.definition().name, "skill_review");
}
