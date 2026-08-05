use std::convert::Infallible;
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use inference_plugin::{AgentToolDefinitions, ToolCall, ToolDefinition};
use margatroid_types::ResourceName;
use serde::Deserialize;
use serde_json::json;
use tool_plugin::{AppToolExt, Tool, ToolCallResult, ToolContext, ToolPlugin, WorldToolExt};

#[derive(Deserialize)]
struct EchoArguments {
    text: String,
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ToolPlugin::default());
    app.register_tool(
        Tool::new(
            ResourceName::new("builtin/echo").unwrap(),
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
        .set_registered_agent_tools(agent, [ResourceName::new("builtin/echo").unwrap()])
        .unwrap();
    assert_eq!(
        app.world()
            .get_component::<AgentToolDefinitions>(agent)
            .unwrap()
            .tools()[0]
            .name,
        "echo"
    );

    app.world().send_tool_call(
        "request",
        agent,
        ToolCall {
            id: "call".into(),
            name: "echo".into(),
            arguments: r#"{"text":"hello"}"#.into(),
        },
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<ToolCallResult>()
            .into_iter()
            .find(|event| event.id == "request")
        {
            assert_eq!(event.agent, agent);
            assert_eq!(event.tool_call_id, "call");
            assert_eq!(event.result.as_deref().unwrap(), "request:call:hello");
            break;
        }
        assert!(Instant::now() < deadline, "tool execution timed out");
        std::thread::yield_now();
    }
}
