use std::convert::Infallible;

use core_plugin::App;
use margatroid_types::{ResourceName, ToolDefinition};
use serde::Deserialize;
use serde_json::json;
use tool_plugin::{AppToolExt, Tool, ToolContext, ToolPlugin, WorldToolExt};

#[derive(Deserialize)]
struct EchoArguments {
    text: String,
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let mut app = App::new();
    app.add_plugin(ToolPlugin::new());
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

    let tool = app
        .world()
        .registered_tool(&ResourceName::new("builtin/echo").unwrap())
        .unwrap();
    assert_eq!(tool.definition().name, "echo");
}
