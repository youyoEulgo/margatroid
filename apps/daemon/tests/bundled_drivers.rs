use agent_driver_loader_plugin::AgentDriverLoaderPlugin;
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
fn bundled_resources() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../resources")
        .canonicalize()
        .unwrap()
}
fn app(drivers: &Path) -> App {
    let home = bundled_resources();
    let mut app = App::new();
    app.world_mut().insert_resource(
        margatroid_types::model_components::ModelRuntimeCatalog::new([
            margatroid_types::model_components::ModelRuntimeInfo {
                id: "openrouter/deepseek-v4.1-flash".into(),
                model: "offline-model".into(),
                api_type: "chat_completions".into(),
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
        .add_plugin(AgentDriverLoaderPlugin::open(drivers).unwrap())
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
fn start(app: &mut App, root: &Path, driver: &str) -> (Entity, Entity) {
    fs::create_dir_all(root.join("project")).unwrap();
    app.world().send_event(StartWorkspace {
        id: "start-bundled-driver".into(),
        definition: WorkspaceDefinition {
            id: id("workspace:local/bundled-driver-test"),
            name: "bundled-driver-test".into(),
            manager: "driver".into(),
            project_root: root.join("project"),
            agents: vec![WorkspaceAgentDefinition {
                id: id("agent:local/driver"),
                name: "driver".into(),
                driver: id(driver),
                home_root: root.join("home"),
                history_path: root.join("history.sqlite"),
            }],
        },
    });
    let result = tick_until(app, "actual driver ready", |world| {
        world
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "start-bundled-driver")
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
    assert!(matches!(agent.state, AgentState::Idle));
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
    command_to(app, "agent:local/driver", request, text, arguments)
}
fn command_to(
    app: &mut App,
    target: &str,
    request: &str,
    text: &str,
    arguments: Vec<serde_json::Value>,
) -> MclValue {
    app.world().send_event(MclCommand {
        id: request.into(),
        source: id("client:webui/bundled-driver-test:1"),
        target: id(target),
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
        replay: None,
        reasoning: None,
        content: Some(content.into()),
        stop_reason: Some(if tool_calls.is_empty() {
            StopReason::Completed
        } else {
            StopReason::ToolCalls
        }),
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
    stop_agents(app, workspace, &[entity]);
}
fn stop_agents(app: &mut App, workspace: Entity, entities: &[Entity]) {
    let handles = entities
        .iter()
        .map(|entity| {
            let agent = app.world().get_component::<Agent>(*entity).unwrap();
            (agent.execution.clone(), agent.mcl.drained.clone())
        })
        .collect::<Vec<_>>();
    let close = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .execution_close();
    app.world().send_event(StopWorkspace {
        id: "stop-bundled-driver".into(),
        workspace,
    });
    let result = tick_until(app, "stop and join", |world| {
        world
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "stop-bundled-driver")
            .map(|event| event.result.clone())
    });
    assert_eq!(result.unwrap(), workspace);
    assert!(!app.world().is_alive(workspace));
    for (entity, (scope, drain)) in entities.iter().zip(handles) {
        assert!(!app.world().is_alive(*entity));
        assert!(scope.is_closed());
        assert_eq!(scope.activity_count(), 0);
        assert!(drain.is_drained());
    }
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
fn exercise_root_restart(drivers: &Path, driver: &str, soul_name: &str) {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(drivers);
    let (workspace, entity) = start(&mut app, temp.path(), driver);
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
    for (name, imported) in [(soul_name, &soul), ("compact", &compact)] {
        let resource = app
            .world()
            .get_component::<ResourceSet<PromptResource>>(entity)
            .unwrap()
            .get(&id(&format!("prompt:local/{name}:latest")))
            .unwrap();
        let content = resource.content.to_string();
        let expected = match resource.role.as_str() {
            "system" => AgentMessage::System { content },
            "user" => AgentMessage::User {
                content,
                images: Vec::new(),
            },
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
        images: Vec::new(),
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
                images: vec![margatroid_types::UserImage {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                }],
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
    let (workspace, entity) = start(&mut app, temp.path(), driver);
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
        images: Vec::new(),
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
fn bundled_coder_driver_offline_lifecycle() {
    exercise_root_restart(
        &bundled_resources().join("agent-drivers"),
        "driver:local/coder:latest",
        "coder-soul",
    );
}

#[test]
fn bundled_reviewer_driver_offline_lifecycle() {
    exercise_root_restart(
        &bundled_resources().join("agent-drivers"),
        "driver:local/reviewer:latest",
        "reviewer-soul",
    );
}

#[test]
fn bundled_coder_and_reviewer_share_workspace_with_isolated_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("project")).unwrap();
    let mut app = app(&bundled_resources().join("agent-drivers"));
    app.world().send_event(StartWorkspace {
        id: "start-pair".into(),
        definition: WorkspaceDefinition {
            id: id("workspace:local/bundled-pair"),
            name: "bundled-pair".into(),
            manager: "coder".into(),
            project_root: root.join("project"),
            agents: ["coder", "reviewer"]
                .into_iter()
                .map(|name| WorkspaceAgentDefinition {
                    id: id(&format!("agent:local/{name}")),
                    name: name.into(),
                    driver: id(&format!("driver:local/{name}:latest")),
                    home_root: root.join(name).join("home"),
                    history_path: root.join(name).join("history.sqlite"),
                })
                .collect(),
        },
    });
    let workspace = tick_until(&mut app, "paired drivers ready", |world| {
        world
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "start-pair")
            .map(|event| event.result.clone())
    })
    .unwrap();
    assert_eq!(app.world().query_with::<Workspace>().result().len(), 1);
    let members = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .map(|(_, entity)| entity)
        .collect::<Vec<_>>();
    assert_eq!(members.len(), 2);
    let find_agent = |name: &str| {
        *members
            .iter()
            .find(|entity| {
                app.world()
                    .get_component::<Agent>(**entity)
                    .unwrap()
                    .resourceid
                    == id(&format!("agent:local/{name}"))
            })
            .unwrap()
    };
    let coder = find_agent("coder");
    let reviewer = find_agent("reviewer");
    let coder_instance = app
        .world()
        .get_component::<Agent>(coder)
        .unwrap()
        .lua
        .instance
        .clone()
        .unwrap();
    let reviewer_instance = app
        .world()
        .get_component::<Agent>(reviewer)
        .unwrap()
        .lua
        .instance
        .clone()
        .unwrap();
    assert_ne!(coder_instance, reviewer_instance);
    assert_eq!(coder_instance.owner, coder);
    assert_eq!(reviewer_instance.owner, reviewer);
    for (name, entity, sandbox, expected_tools) in [
        (
            "coder",
            coder,
            "workspace-write",
            vec![
                "review",
                "list_dir",
                "read_file",
                "read_image",
                "write_file",
                "edit",
                "grep",
                "glob",
                "bash",
            ],
        ),
        (
            "reviewer",
            reviewer,
            "read-only",
            vec!["review", "project_context", "list_dir"],
        ),
    ] {
        let agent = app.world().get_component::<Agent>(entity).unwrap();
        assert!(matches!(agent.state, AgentState::Idle));
        assert!(agent.lua.ready.is_none());
        assert_eq!(agent.workspace, id("workspace:local/bundled-pair"));
        let selected = agent.sandbox.as_ref().unwrap().clone();
        assert_eq!(
            selected.resource_id,
            id(&format!("sandbox:local/{sandbox}:latest"))
        );
        let policy: serde_json::Value = serde_json::from_str(&selected.policy).unwrap();
        if name == "reviewer" {
            assert_eq!(
                policy["filesystem"]["denyWrite"],
                serde_json::json!(["/**"])
            );
            assert!(policy["filesystem"]["allowWrite"].is_null());
        } else {
            assert!(policy["filesystem"]["allowWrite"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(".")));
            assert!(policy["filesystem"]["denyWrite"].is_null());
        }
        let target = format!("agent:local/{name}");
        let MclValue::Array(BlockInner::Tool(tools)) = command_to(
            &mut app,
            &target,
            &format!("{name}-visible"),
            "GET session.visible",
            vec![],
        ) else {
            panic!("{name} must have typed visible tools")
        };
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.alias.as_str())
                .collect::<Vec<_>>(),
            expected_tools
        );
        for tool in &tools {
            let resource_name = match tool.alias.as_str() {
                "review" => "code-review",
                "project_context" => "project-context",
                "list_dir" => "list-directory",
                "read_file" => "read-file",
                "read_image" => "read-image",
                "write_file" => "write-file",
                alias => alias,
            };
            assert_eq!(
                tool.resource_id,
                id(&format!("tool:local/{resource_name}:latest"))
            );
        }
        let MclValue::Array(BlockInner::Sandbox(sandboxes)) = command_to(
            &mut app,
            &target,
            &format!("{name}-sandbox"),
            "GET session.sandbox_activated",
            vec![],
        ) else {
            panic!("{name} must have a typed active sandbox")
        };
        assert_eq!(sandboxes, vec![selected]);
        assert!(messages(command_to(
            &mut app,
            &target,
            &format!("{name}-empty-context"),
            "GET session.conversation",
            vec![],
        ))
        .is_empty());
        command_to(
            &mut app,
            &target,
            &format!("{name}-inject"),
            "INJECT ? TO entry.pending",
            vec![serde_json::json!([{"type":"user", "content":format!("private {name} request")}])],
        );
        command_to(
            &mut app,
            &target,
            &format!("{name}-wake"),
            "EMIT EFFECT wake",
            vec![],
        );
        let request = inference(&mut app, entity);
        assert_eq!(request.tools, tools);
        respond(
            &request,
            assistant(&format!("private {name} answer"), vec![]),
            StopReason::Completed,
        );
        turn_done(&mut app, entity);
    }
    let mut histories = Vec::new();
    for (name, entity) in [("coder", coder), ("reviewer", reviewer)] {
        let expected = vec![
            AgentMessage::User {
                images: Vec::new(),
                content: format!("private {name} request"),
            },
            assistant(&format!("private {name} answer"), vec![]),
        ];
        assert_eq!(
            messages(command_to(
                &mut app,
                &format!("agent:local/{name}"),
                &format!("{name}-context"),
                "GET session.conversation",
                vec![],
            )),
            expected
        );
        assert_eq!(history_messages(&app, entity), expected);
        let agent = app.world().get_component::<Agent>(entity).unwrap();
        assert!(matches!(agent.state, AgentState::Idle));
        histories.push((name, agent.history.list().unwrap()));
    }
    stop_agents(&mut app, workspace, &[coder, reviewer]);
    assert!(app.world().query_with::<Agent>().result().is_empty());
    for (name, expected) in histories {
        let database = root.join(name).join("history.sqlite");
        assert!(database.is_file());
        assert_eq!(
            SqliteAgentDatabase::open(database).unwrap().list().unwrap(),
            expected
        );
    }
}

#[test]
fn bundled_driver_abort_keeps_history_out_of_context_and_accepts_next_turn() {
    use margatroid_types::agent_events::{AbortAgentTurn, AbortAgentTurnResult};
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(&bundled_resources().join("agent-drivers"));
    let (workspace, entity) = start(&mut app, temp.path(), "driver:local/coder:latest");
    inject(&mut app, "abort-user", "please work");
    let request = inference(&mut app, entity);
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let turn = agent.turn.clone().unwrap();
    app.world().send_event(AbortAgentTurn {
        id: "abort-test".into(),
        agent: entity,
        resource_id: agent.resourceid.clone(),
        workspace: agent.workspace.clone(),
        instance: agent.lua.instance.clone().unwrap(),
        expected_turn_id: turn,
    });
    let abort = tick_until(&mut app, "abort accepted", |world| {
        world
            .event_reader::<AbortAgentTurnResult>()
            .into_iter()
            .find(|event| event.id == "abort-test")
            .cloned()
    });
    assert!(abort.result.is_ok(), "{abort:?}");
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .turn
        .is_none());
    let partial = AgentMessage::Assistant {
        reasoning: Some("Partial thought before cancellation".into()),
        replay: None,
        content: Some("Partial answer before cancellation".into()),
        tool_calls: vec![],
        usage: None,
        stop_reason: Some(StopReason::Cancelled),
    };
    if let Some(reply) = request.reply.lock().unwrap().take() {
        reply
            .send(Ok(InferenceResponse {
                message: partial.clone(),
                stop_reason: StopReason::Cancelled,
            }))
            .unwrap();
    }
    turn_done(&mut app, entity);
    assert!(matches!(
        app.world().get_component::<Agent>(entity).unwrap().state,
        AgentState::Idle
    ));
    let history = history_messages(&app, entity);
    assert!(history.contains(&partial));
    let context = messages(command(
        &mut app,
        "abort-context",
        "GET session.conversation",
        vec![],
    ));
    assert!(!context.iter().any(|message| matches!(
        message,
        AgentMessage::Assistant {
            stop_reason: Some(StopReason::Cancelled),
            ..
        }
    )));
    inject(&mut app, "after-abort", "new turn");
    let next = inference(&mut app, entity);
    assert_ne!(next.turn, request.turn);
    assert!(!next.messages.contains(&partial));
    respond(&next, assistant("recovered", vec![]), StopReason::Completed);
    turn_done(&mut app, entity);
    stop(&mut app, workspace, entity);
}

#[test]
fn bundled_driver_preserves_user_images_through_lua_history_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let drivers = bundled_resources().join("agent-drivers");
    let mut app = app(&drivers);
    let (workspace, entity) = start(&mut app, temp.path(), "driver:local/coder:latest");
    let user: AgentMessage = serde_json::from_value(serde_json::json!({
        "type":"user", "content":"Inspect this pixel",
        "images":[{"media_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a6X8AAAAASUVORK5CYII="}]
    })).unwrap();
    command(
        &mut app,
        "image-inject",
        "INJECT ? TO entry.pending",
        vec![serde_json::json!([user.clone()])],
    );
    command(&mut app, "image-wake", "EMIT EFFECT wake", vec![]);
    let first = inference(&mut app, entity);
    assert!(first.messages.contains(&user));
    respond(
        &first,
        assistant("Pixel received", vec![]),
        StopReason::Completed,
    );
    turn_done(&mut app, entity);
    assert_eq!(history_messages(&app, entity)[0], user);
    let records = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap();
    let record = records
        .iter()
        .find(|record| record.kind == "message")
        .unwrap();
    assert_eq!(record.content, "Inspect this pixel");
    assert_eq!(record.payload["images"][0]["media_type"], "image/png");
    assert!(!record.content.contains("iVBOR"));
    stop(&mut app, workspace, entity);
    let (workspace, entity) = start(&mut app, temp.path(), "driver:local/coder:latest");
    assert_eq!(history_messages(&app, entity)[0], user);
    assert_eq!(
        messages(command(
            &mut app,
            "image-restored",
            "GET session.conversation",
            vec![]
        ))[0],
        user
    );
    inject(&mut app, "image-next-turn", "Still remember the image?");
    let next = inference(&mut app, entity);
    assert!(next.messages.contains(&user));
    respond(
        &next,
        assistant("History retained", vec![]),
        StopReason::Completed,
    );
    turn_done(&mut app, entity);
    stop(&mut app, workspace, entity);
}
