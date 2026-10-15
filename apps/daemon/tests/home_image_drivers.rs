use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::{Agent, AgentPlugin, AgentState};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, World};
use lua_runtime_plugin::LuaRuntimePlugin;
use margatroid_types::block_storage::{
    BlockExpression, BlockRange, BlockSelector, BlockSource, BlockStorageContext,
    BlockStorageOperation, BlockStorageRequest, StoredBlockInner, StoredBlockResult,
};
use margatroid_types::block_types::{BlockInner, BlockPath};
use margatroid_types::history_components::AgentHistoryStore;
use margatroid_types::inference_events::InferenceRequest;
use margatroid_types::mcl_events::{MclCommand, MclCommandResult, MclInstruction, MclValue};
use margatroid_types::message_types::{AgentMessage, InferenceResponse, StopReason, ToolCall};
use margatroid_types::resource_components::PromptResource;
use margatroid_types::tool_events::{ToolExecuteRequest, ToolExecuteResponse};
use margatroid_types::workspace_components::{
    Workspace, WorkspaceAgentDefinition, WorkspaceDefinition,
};
use margatroid_types::workspace_events::{
    StartWorkspace, StartWorkspaceResult, StopWorkspace, StopWorkspaceResult,
};
use mcl_plugin::MclPlugin;
use memory_plugin::{MemoryPlugin, SqliteAgentDatabase};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use workspace_plugin::WorkspacePlugin;

fn id(text: &str) -> ResourceID {
    ResourceID::parse(text).unwrap()
}
fn global_home() -> PathBuf {
    std::env::var_os("MARGATROID_IMAGE_TEST_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME").expect("HOME or MARGATROID_IMAGE_TEST_HOME required"),
            )
            .join(".margatroid")
        })
}
fn app(images: &Path) -> App {
    let home = global_home();
    let mut app = App::new();
    app.world_mut().insert_resource(
        margatroid_types::model_components::ModelRuntimeCatalog::new([
            margatroid_types::model_components::ModelRuntimeInfo {
                id: "openrouter/deepseek-v4.1-flash".into(),
                model: "offline-model".into(),
                api_type: "deepseek".into(),
                provider: Some("offline".into()),
                context_window_tokens: 1000000,
                thinking: Some("enabled".into()),
                reasoning_effort: Some("high".into()),
            },
        ]),
    );
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(tool_resource_plugin::ToolResourcePlugin::open(&home).unwrap())
        .add_plugin(prompt_resource_plugin::PromptResourcePlugin::open(&home).unwrap())
        .add_plugin(sandbox_resource_plugin::SandboxResourcePlugin::open(&home).unwrap())
        .add_plugin(AgentImageLoaderPlugin::open(images).unwrap())
        .add_plugin(MemoryPlugin::new())
        .add_plugin(MclPlugin::default())
        .add_plugin(LuaRuntimePlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(WorkspacePlugin::default());
    app
}
fn stored(agent: &Agent, block: &str, field: &str) -> StoredBlockInner {
    let storage = agent.database.block_storage();
    let request = BlockStorageRequest {
        context: BlockStorageContext {
            agent: agent.resourceid.clone(),
            instance: agent.lua.instance.as_ref().unwrap().clone(),
        },
        operation: BlockStorageOperation::Get {
            expressions: vec![BlockExpression::Sources(vec![BlockSource::Selector(
                BlockSelector {
                    path: BlockPath {
                        block_id: block.into(),
                        inner_id: field.into(),
                    },
                    range: BlockRange::All,
                },
            )])],
        },
        cancel: Default::default(),
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(storage.execute(request))
        .unwrap();
    let StoredBlockResult::Arrays(mut arrays) = result else {
        panic!("expected arrays")
    };
    arrays.remove(0)
}
fn diagnostics(world: &World) -> String {
    let agents = world.query_with::<Agent>().result().into_iter().map(|entity| {
        let agent = world.get_component::<Agent>(entity).unwrap();
        format!("{entity:?}{} {:?} turn={:?} activities={} queue={:?} pending={:?} drain={} wake={:?}",
            agent.resourceid, agent.state, agent.turn, agent.execution.activity_count(),
            agent.mcl.commands, agent.mcl.pending, agent.mcl.drained.is_drained(), agent.lua.wake)
    }).collect::<Vec<_>>();
    let workspaces = world
        .query_with::<Workspace>()
        .result()
        .into_iter()
        .map(|entity| {
            format!(
                "{entity:?} {:?}",
                world.get_component::<Workspace>(entity).unwrap().state()
            )
        })
        .collect::<Vec<_>>();
    format!(
        "agents={agents:?}; workspaces={workspaces:?}; normal_events={} pending_events={}",
        world.event_snapshot().normal_event_count,
        world.event_snapshot().pending_event_count
    )
}
fn tick_until<T>(app: &mut App, label: &str, mut inspect: impl FnMut(&World) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        let exceptions = app
            .world()
            .event_reader::<margatroid_types::lua_runtime_events::DriverLuaVmException>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            exceptions.is_empty(),
            "{label}: driver exceptions={exceptions:?}"
        );
        if let Some(result) = inspect(app.world()) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "{label} timed out:{}",
            diagnostics(app.world())
        );
        std::thread::yield_now();
    }
}
fn start(app: &mut App, root: &Path, image: &str) -> (Entity, Entity) {
    fs::create_dir_all(root.join("project")).unwrap();
    app.world().send_event(StartWorkspace {
        id: "start-home-image".into(),
        definition: WorkspaceDefinition {
            id: id("workspace:local/home-image-test"),
            name: "home-image-test".into(),
            manager: "driver".into(),
            project_root: root.join("project"),
            agents: vec![WorkspaceAgentDefinition {
                id: id("agent:local/driver"),
                name: "driver".into(),
                image: id(image),
                home_root: root.join("home"),
                history_path: root.join("history.sqlite"),
            }],
        },
    });
    let result = tick_until(app, "actual image ready", |world| {
        world
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "start-home-image")
            .map(|event| event.result.clone())
    });
    let workspace = result
        .unwrap_or_else(|error| panic!("startup failed: {error:?};{}", diagnostics(app.world())));
    let entity = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1;
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Running));
    assert!(agent.lua.ready.is_none());
    assert_eq!(agent.lua.instance.as_ref().unwrap().owner, entity);
    (workspace, entity)
}
fn command(
    app: &mut App,
    request: &str,
    text: &str,
    arguments: Vec<serde_json::Value>,
) -> MclValue {
    app.world().send_event(MclCommand {
        id: request.into(),
        source: id("client:webui/home-image-test:1"),
        target: id("agent:local/driver"),
        command: MclInstruction {
            text: text.into(),
            arguments,
        },
    });
    let result = tick_until(app, text, |world| {
        world
            .event_reader::<MclCommandResult>()
            .into_iter()
            .find(|event| event.id == request)
            .cloned()
    });
    assert!(result.history_error.is_none(), "{result:?}");
    result
        .result
        .unwrap_or_else(|error| panic!("{text}: {error:?};{}", diagnostics(app.world())))
}
fn messages(value: MclValue) -> Vec<AgentMessage> {
    match value {
        MclValue::Array(BlockInner::Message(messages)) => messages,
        other => panic!("expected messages: {other:?}"),
    }
}
fn assistant(content: &str, tool_calls: Vec<ToolCall>) -> AgentMessage {
    AgentMessage::Assistant {
        reasoning: None,
        content: Some(content.into()),
        tool_calls,
        usage: None,
    }
}
fn inference(app: &mut App, entity: Entity) -> InferenceRequest {
    tick_until(app, "mock inference request", |world| {
        world
            .event_reader::<InferenceRequest>()
            .into_iter()
            .find(|request| request.agent == entity)
            .cloned()
    })
}
fn respond(request: &InferenceRequest, message: AgentMessage, stop_reason: StopReason) {
    assert!(!request.cancel.is_cancelled());
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .expect("inference reply consumed once")
        .send(Ok(InferenceResponse {
            message,
            stop_reason,
        }))
        .unwrap();
}
fn inject(app: &mut App, suffix: &str, content: &str) {
    command(
        app,
        &format!("inject-{suffix}"),
        "INJECT ? TO entry.pending",
        vec![serde_json::json!([{"type":"user", "content":content}])],
    );
    command(app, &format!("wake-{suffix}"), "EMIT EFFECT wake", vec![]);
}
fn history_messages(app: &App, entity: Entity) -> Vec<AgentMessage> {
    app.world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap()
        .into_iter()
        .filter(|record| record.kind == "message")
        .map(|record| {
            let mut payload = record.payload;
            let field = if payload["type"] == "error" {
                "message"
            } else {
                "content"
            };
            if payload["type"] != "inject" && payload.get(field).is_none() {
                payload[field] = serde_json::Value::String(record.content);
            }
            serde_json::from_value(payload).unwrap()
        })
        .collect()
}
fn turn_done(app: &mut App, entity: Entity) {
    tick_until(app, "turn completion and driver wait", |world| {
        assert!(
            world
                .event_reader::<InferenceRequest>()
                .into_iter()
                .all(|request| request.agent != entity),
            "unexpected inference while waiting for idle"
        );
        assert!(
            world
                .event_reader::<ToolExecuteRequest>()
                .into_iter()
                .all(|request| request.agent != entity),
            "unexpected tool execution while waiting for idle"
        );
        let agent = world.get_component::<Agent>(entity)?;
        let pending = stored(agent, "entry", "pending");
        (agent.turn.is_none()
            && pending.is_empty()
            && agent.execution.activity_count() == 0
            && agent.mcl.commands.is_empty()
            && agent.mcl.pending.is_none()
            && {
                let wake = agent.lua.wake.lock().unwrap();
                wake.waiting && wake.sender.is_some() && !wake.pending_wake
            })
        .then_some(())
    });
}
fn stop(app: &mut App, workspace: Entity, entity: Entity) {
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let scope = agent.execution.clone();
    let drain = agent.mcl.drained.clone();
    let close = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .execution_close();
    app.world().send_event(StopWorkspace {
        id: "stop-home-image".into(),
        workspace,
    });
    let result = tick_until(app, "stop and join", |world| {
        world
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "stop-home-image")
            .map(|event| event.result.clone())
    });
    assert_eq!(result.unwrap(), workspace);
    assert!(!app.world().is_alive(workspace));
    assert!(!app.world().is_alive(entity));
    assert!(scope.is_closed());
    assert_eq!(scope.activity_count(), 0);
    assert!(drain.is_drained());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), close.close())
                .await
                .unwrap()
                .unwrap();
        });
}
fn exercise_root_restart(images: &Path, image: &str) {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(images);
    let (workspace, entity) = start(&mut app, temp.path(), image);
    assert!(history_messages(&app, entity).is_empty());
    let MclValue::Array(BlockInner::Tool(review)) =
        command(&mut app, "root-review", "GET review", vec![])
    else {
        panic!("review must be an ordinary tool snapshot")
    };
    assert_eq!(review.len(), 1);
    assert_eq!(review[0].resource_id, id("tool:local/code-review:latest"));
    assert_eq!(review[0].alias, "review");
    let soul = messages(command(&mut app, "root-soul", "GET soul", vec![]));
    let compact = messages(command(&mut app, "root-compact", "GET compact", vec![]));
    assert!(!soul.is_empty());
    assert!(!compact.is_empty());
    for (name, imported) in [("coder-soul", &soul), ("compact", &compact)] {
        let resource = app
            .world()
            .get_component::<ResourceSet<PromptResource>>(entity)
            .unwrap()
            .get(&id(&format!("prompt:local/{name}:latest")))
            .unwrap();
        let content = resource.content.to_string();
        let expected = match resource.role.as_str() {
            "system" => AgentMessage::System { content },
            "user" => AgentMessage::User { content },
            "assistant" => assistant(&content, vec![]),
            role => panic!("unexpected prompt role {role}"),
        };
        assert_eq!(
            imported,
            &vec![expected],
            "IMPORT must preserve the resource role and body"
        );
    }
    assert_eq!(
        messages(command(
            &mut app,
            "root-instructions",
            "GET msg.instructions",
            vec![]
        )),
        soul
    );
    assert_eq!(
        messages(command(
            &mut app,
            "root-compact-prompt",
            "GET msg.compact_prompt",
            vec![]
        )),
        compact
    );
    command(
        &mut app,
        "root-select-tools",
        "INJECT list_dir TO session.visible",
        vec![],
    );
    command(
        &mut app,
        "root-select-sandbox",
        "INJECT read_only TO session.sandbox_activated",
        vec![],
    );
    command(
        &mut app,
        "root-use-sandbox",
        "EMIT EFFECT sandbox_use (session.sandbox_activated)",
        vec![],
    );
    let visible = command(&mut app, "root-visible", "GET session.visible", vec![]);
    let sandbox = command(
        &mut app,
        "root-sandbox",
        "GET session.sandbox_activated",
        vec![],
    );
    let MclValue::Array(BlockInner::Tool(tools)) = &visible else {
        panic!("typed tools required")
    };
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].resource_id, id("tool:local/list-directory:latest"));
    assert_eq!(tools[0].alias, "list_dir");
    let selected = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .sandbox
        .clone()
        .unwrap();
    assert_eq!(selected.resource_id, id("sandbox:local/read-only:latest"));
    inject(&mut app, "root-first", "offline persistent user");
    let first = inference(&mut app, entity);
    assert!(first.turn.is_some());
    assert_eq!(first.tools, *tools);
    let user = AgentMessage::User {
        content: "offline persistent user".into(),
    };
    assert_eq!(first.messages, [soul.clone(), vec![user.clone()]].concat());
    let calls = (0..2)
        .map(|index| ToolCall {
            id: format!("root-offline-call-{index}"),
            tool_name: "list_dir".into(),
            arguments: "{}".into(),
        })
        .collect::<Vec<_>>();
    let batch = assistant("offline tool request", calls.clone());
    respond(&first, batch.clone(), StopReason::ToolCalls);
    let mut seen = BTreeSet::new();
    let mut tool_results = Vec::new();
    tick_until(&mut app, "root mock tool batch", |world| {
        for tool in world.event_reader::<ToolExecuteRequest>() {
            if tool.agent != entity || !seen.insert(tool.call.id.clone()) {
                continue;
            }
            assert!(calls.contains(&tool.call));
            assert_eq!(tool.turn, first.turn);
            assert_eq!(tool.resource, tools[0]);
            assert!(!tool.cancel.is_cancelled());
            let tool_result = AgentMessage::ToolCall {
                resource_id: Some(tool.resource.resource_id.clone()),
                tool_call_id: tool.call.id.clone(),
                tool_name: tool.call.tool_name.clone(),
                content: format!("offline mock result {}", tool.call.id),
                failed: false,
            };
            tool.reply
                .lock()
                .unwrap()
                .take()
                .expect("tool reply consumed once")
                .send(Ok(ToolExecuteResponse {
                    message: tool_result.clone(),
                }))
                .unwrap();
            tool_results.push(tool_result);
        }
        (seen.len() == calls.len()).then_some(())
    });
    let followup = inference(&mut app, entity);
    assert_eq!(followup.turn, first.turn);
    assert_eq!(followup.tools, first.tools);
    for result in &tool_results {
        assert_eq!(
            followup.messages.iter().filter(|message| *message == result).count(),
            1,
            "all results from the assistant's two tool calls must be in context before the next inference"
        );
    }
    assert_eq!(
        followup.messages,
        [
            soul.clone(),
            vec![user.clone(), batch.clone()],
            tool_results.clone(),
        ]
        .concat()
    );
    let answer = assistant("offline final response", vec![]);
    respond(&followup, answer.clone(), StopReason::Completed);
    turn_done(&mut app, entity);
    let conversation = [vec![user, batch], tool_results, vec![answer]].concat();
    assert_eq!(
        messages(command(
            &mut app,
            "root-context",
            "GET session.conversation",
            vec![]
        )),
        conversation
    );
    assert_eq!(history_messages(&app, entity), conversation);
    command(
        &mut app,
        "root-stale-entry",
        "INJECT ? TO entry.pending",
        vec![serde_json::json!([{"type":"user", "content":"must not replay after restart"}])],
    );
    let instance = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .lua
        .instance
        .clone()
        .unwrap();
    stop(&mut app, workspace, entity);
    let saved = SqliteAgentDatabase::open(temp.path().join("history.sqlite"))
        .unwrap()
        .list()
        .unwrap();
    let (workspace, entity) = start(&mut app, temp.path(), image);
    assert_ne!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .lua
            .instance
            .as_ref()
            .unwrap(),
        &instance
    );
    assert_eq!(
        command(
            &mut app,
            "root-reopen",
            "CREATE BLOCK session (conversation MESSAGE, visible TOOL, sandbox_activated SANDBOX)",
            vec![]
        ),
        MclValue::BlockOpened {
            block_id: "session".into(),
            created: false,
            temporary: false
        }
    );
    assert_eq!(
        messages(command(
            &mut app,
            "root-restarted-context",
            "GET session.conversation",
            vec![]
        )),
        conversation
    );
    assert_eq!(
        command(
            &mut app,
            "root-restarted-tools",
            "GET session.visible",
            vec![]
        ),
        visible
    );
    assert_eq!(
        command(
            &mut app,
            "root-restarted-sandbox",
            "GET session.sandbox_activated",
            vec![]
        ),
        sandbox
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .sandbox
            .as_ref()
            .unwrap(),
        &selected
    );
    assert!(messages(command(
        &mut app,
        "root-restarted-pending",
        "GET entry.pending",
        vec![]
    ))
    .is_empty());
    assert_eq!(
        messages(command(
            &mut app,
            "root-restarted-instructions",
            "GET msg.instructions",
            vec![]
        )),
        soul
    );
    assert_eq!(
        messages(command(
            &mut app,
            "root-restarted-compact",
            "GET msg.compact_prompt",
            vec![]
        )),
        compact
    );
    assert_eq!(history_messages(&app, entity), conversation);
    let history = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap();
    assert_eq!(&history[..saved.len()], saved.as_slice());
    turn_done(&mut app, entity);
    inject(&mut app, "root-after-restart", "offline after restart");
    let restarted = inference(&mut app, entity);
    assert_eq!(restarted.tools, *tools);
    let new_user = AgentMessage::User {
        content: "offline after restart".into(),
    };
    assert_eq!(
        restarted.messages,
        [soul, conversation.clone(), vec![new_user.clone()]].concat()
    );
    let new_answer = assistant("offline restarted answer", vec![]);
    respond(&restarted, new_answer.clone(), StopReason::Completed);
    turn_done(&mut app, entity);
    assert_eq!(
        messages(command(
            &mut app,
            "root-final-context",
            "GET session.conversation",
            vec![]
        )),
        [conversation, vec![new_user, new_answer]].concat()
    );
    stop(&mut app, workspace, entity);
}
#[test]
#[ignore = "requires read-only global-home resource packages; uses the current root sample driver"]
fn root_sample_driver_with_coder_manifest_offline_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("local/coder/latest");
    fs::create_dir_all(&directory).unwrap();
    fs::copy(
        global_home().join("agent-images/local/coder/latest/agent.toml"),
        directory.join("agent.toml"),
    )
    .unwrap();
    let root_driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../base.lua");
    fs::copy(root_driver, directory.join("base.lua")).unwrap();
    exercise_root_restart(temp.path(), "image:local/coder:latest");
}
