use super::*;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use lua_runtime_core::{DriverLuaVmInstance, DriverWakeSlot};
use tool_resource_core::ToolEntry;

fn fixture(url: String, format: &str) -> (App, Entity) {
    let mut selected = route(url);
    selected.api_type = format.into();
    if format == "messages" {
        selected.provider = "anthropic".into();
    }
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(crate::InferencePlugin {
            schedule: RuntimePlugin::UPDATE.into(),
            routes: inference_core::ModelRoutes::new(BTreeMap::from([("route".into(), selected)])),
        });
    let agent = app.world_mut().spawn();
    app.world_mut().insert_component(
        agent,
        agent_core::Agent::new(
            "test".into(),
            agent,
            AgentState::Working,
            Some("turn".into()),
            test_agent_context(),
        ),
    );
    app.world_mut()
        .insert_component(agent, Address::new("agent:test/agent").unwrap());
    app.world_mut().insert_component(
        agent,
        DriverLuaVm {
            instance: DriverLuaVmInstance {
                owner: agent,
                start_id: "driver-fixture".into(),
            },
            wake: DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(None)),
            ended: None,
            cancel: CancellationToken::new(),
        },
    );
    app.world_mut()
        .insert_component(agent, ActiveInference::default());
    (app, agent)
}

fn entry(alias: &str, owner: Option<&str>) -> ToolEntry {
    ToolEntry {
        // No tool files exist here. HTTP definitions must come only from entries.
        path: std::env::temp_dir()
            .join("inference-entry-without-files")
            .join(alias)
            .join("tool.toml"),
        description: format!("Description for {alias}"),
        input_schema: json!({"type":"object", "properties":{"value":{"type":"string"}}, "required":["value"]}),
        owner_task_id: owner.map(str::to_owned),
        binding: Some(json!({"private_binding":"must never reach the provider"})),
    }
}

fn submit(
    app: &App,
    agent: Entity,
    id: &str,
) -> oneshot::Receiver<Result<InferenceResponse, InferenceError>> {
    let mut input = request();
    input.id = id.into();
    input.agent = agent;
    let (sender, receiver) = oneshot::channel();
    input.reply = Arc::new(Mutex::new(Some(sender)));
    app.world().send_event(input);
    receiver
}

async fn response(
    app: &mut App,
    agent: Entity,
    mut reply: oneshot::Receiver<Result<InferenceResponse, InferenceError>>,
) -> InferenceResponse {
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            app.tick();
            match reply.try_recv() {
                Ok(result) => break result.unwrap(),
                Err(oneshot::error::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(failed) => panic!("inference reply lost: {failed}"),
            }
        }
    })
    .await
    .unwrap();
    let active = app
        .world()
        .get_component::<ActiveInference>(agent)
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .clone();
    tokio::time::timeout(Duration::from_secs(3), active.wait_finished())
        .await
        .unwrap()
        .unwrap();
    result
}

fn completed_trace(format: &str) -> String {
    let events = match format {
        "messages" => vec![
            json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ok"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            json!({"type":"message_stop"}),
        ],
        "responses" => vec![
            json!({"type":"response.created"}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","content":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"ok"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"ok"}]}}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ],
        _ => {
            return format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]})
            );
        }
    };
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn assert_definitions(body: &Value, format: &str, aliases: &[&str]) {
    let expected = aliases.iter().map(|alias| {
        let item = entry(alias, None);
        match format {
            "messages" => json!({"name": alias, "description": item.description, "input_schema": item.input_schema}),
            "responses" => json!({"type":"function", "name": alias, "description": item.description, "parameters": item.input_schema, "strict":false}),
            _ => json!({"type":"function", "function":{"name":alias, "description":item.description, "parameters":item.input_schema}}),
        }
    }).collect::<Vec<_>>();
    assert_eq!(body["tools"], json!(expected));
    assert!(!body.to_string().contains("private_binding"));
    assert!(!body.to_string().contains("owner_task_id"));
    assert!(!body.to_string().contains("tool.toml"));
}

#[tokio::test]
async fn real_http_uses_all_current_entries_at_acceptance_and_refreshes_each_request() {
    for format in ["chat_completions", "messages", "responses"] {
        let (url, chunks, server, captured) = http_server_with_request().await;
        let (mut app, agent) = fixture(url, format);
        let mut tools = Tools::new(test_probe());
        tools
            .insert("baseline".into(), entry("baseline", None))
            .unwrap();
        app.world_mut().insert_component(agent, tools);
        let reply = submit(&app, agent, "first");
        // Even an entry imported after event emission belongs to this request
        // if it exists when the inference system actually accepts the event.
        app.world_mut()
            .get_component_mut::<Tools>(agent)
            .unwrap()
            .insert("dynamic".into(), entry("dynamic", Some("mcp-owner")))
            .unwrap();
        chunks.send(completed_trace(format)).await.unwrap();
        drop(chunks);
        assert_eq!(
            response(&mut app, agent, reply).await.content.as_deref(),
            Some("ok")
        );
        assert_definitions(&captured.await.unwrap(), format, &["baseline", "dynamic"]);
        server.await.unwrap();

        let (url, chunks, server, captured) = http_server_with_request().await;
        let routes = app.world_mut().get_resource_mut::<ModelRoutes>().unwrap();
        let mut entries = routes.entries().clone();
        entries.get_mut("route").unwrap().base_url = url;
        routes.replace(entries, &[]);
        let tools = app.world_mut().get_component_mut::<Tools>(agent).unwrap();
        tools.remove_owner("mcp-owner");
        tools
            .insert("later".into(), entry("later", Some("next-owner")))
            .unwrap();
        let reply = submit(&app, agent, "second");
        chunks.send(completed_trace(format)).await.unwrap();
        drop(chunks);
        assert_eq!(
            response(&mut app, agent, reply).await.content.as_deref(),
            Some("ok")
        );
        assert_definitions(&captured.await.unwrap(), format, &["baseline", "later"]);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn real_http_preserves_large_catalogs_and_long_descriptions_in_every_format() {
    for format in ["chat_completions", "messages", "responses"] {
        let (url, chunks, server, captured) = http_server_with_request().await;
        let (mut app, agent) = fixture(url, format);
        let mut tools = Tools::new(test_probe());
        for index in 0..1024 {
            let alias = format!("catalog_{index:04}");
            let mut item = entry(&alias, Some("large-catalog-owner"));
            item.description = format!("{}-complete-{index}", "d".repeat(16 * 1024));
            tools.insert(alias, item).unwrap();
        }
        app.world_mut().insert_component(agent, tools);
        let reply = submit(&app, agent, "large-catalog");
        chunks.send(completed_trace(format)).await.unwrap();
        drop(chunks);
        assert_eq!(
            response(&mut app, agent, reply).await.content.as_deref(),
            Some("ok")
        );
        let body = captured.await.unwrap();
        let definitions = body["tools"].as_array().unwrap();
        assert_eq!(definitions.len(), 1024);
        for (index, definition) in definitions.iter().enumerate() {
            let definition = if format == "chat_completions" {
                &definition["function"]
            } else {
                definition
            };
            assert_eq!(definition["name"], format!("catalog_{index:04}"));
            assert_eq!(
                definition["description"],
                format!("{}-complete-{index}", "d".repeat(16 * 1024))
            );
            let schema = if format == "messages" {
                &definition["input_schema"]
            } else {
                &definition["parameters"]
            };
            assert_eq!(schema["required"], json!(["value"]));
        }
        server.await.unwrap();
    }
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}

fn test_probe<T>() -> client_core::UpdateProbe<T> {
    client_core::UpdateProbe::new(|_| {})
}
