use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::{Agent, AgentPlugin, AgentState};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, World};
use lua_runtime_plugin::LuaRuntimePlugin;
use margatroid_types::block_types::{BlockInner, BlockPath};
use margatroid_types::history_components::AgentHistoryStore;
use margatroid_types::inference_events::{InferenceError, InferenceErrorKind, InferenceRequest};
use margatroid_types::mcl_events::{MclCommand, MclCommandResult, MclInstruction, MclValue};
use margatroid_types::message_types::{AgentMessage, InferenceResponse, StopReason, ToolCall};
use margatroid_types::resource_components::{
    CallableDefinitionSnapshot, PromptResource, SandboxResource, SkillResource, ToolResource,
};
use margatroid_types::tool_events::{ToolExecuteRequest, ToolExecuteResponse};
use margatroid_types::workspace_components::{
    Workspace, WorkspaceAgentDefinition, WorkspaceDefinition,
};
use margatroid_types::workspace_events::{
    StartWorkspace, StartWorkspaceResult, StopWorkspace, StopWorkspaceResult,
};
use mcl_plugin::MclPlugin;
use memory_plugin::{MemoryPlugin, SqliteHistoryStore};
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
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(tool_resource_plugin::ToolResourcePlugin::open(&home).unwrap())
        .add_plugin(skill_resource_plugin::SkillResourcePlugin::open(&home).unwrap())
        .add_plugin(hook_resource_plugin::HookResourcePlugin::open(&home).unwrap())
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
fn inject_messages(app: &mut App, suffix: &str, messages: &[AgentMessage]) {
    command(
        app,
        &format!("inject-{suffix}"),
        "INJECT ? TO entry.pending",
        vec![serde_json::to_value(messages).unwrap()],
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
        .map(|record| serde_json::from_value(record.payload).unwrap())
        .collect()
}
fn exercise_entry_dispatch(app: &mut App, entity: Entity) {
    let before = history_messages(app, entity);
    let outer_assistant = assistant("external assistant without calls", vec![]);
    let outer_error = AgentMessage::Error {
        message: "external error".into(),
    };
    inject_messages(app, "outer-assistant", &[outer_assistant.clone()]);
    turn_done(app, entity);
    assert_eq!(
        history_messages(app, entity),
        [before.clone(), vec![outer_assistant.clone()]].concat()
    );
    inject_messages(app, "outer-error", &[outer_error.clone()]);
    turn_done(app, entity);
    let outer_tool = AgentMessage::ToolCall {
        resource_id: Some(id("tool:local/list-directory:latest")),
        tool_call_id: "external-result".into(),
        tool_name: "list_dir".into(),
        content: "external tool result triggers inference".into(),
        failed: false,
    };
    inject_messages(app, "outer-tool", &[outer_tool.clone()]);
    let request = inference(app, entity);
    assert!(
        request.turn.is_none(),
        "tool result alone must not start a user turn"
    );
    assert_eq!(request.messages.last(), Some(&outer_tool));
    let error_before = history_messages(app, entity);
    request
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Err(InferenceError::new(
            InferenceErrorKind::ExecutionFailed,
            "offline inference failure",
        )))
        .unwrap();
    turn_done(app, entity);
    let after_error = history_messages(app, entity);
    assert_eq!(&after_error[..error_before.len()], error_before.as_slice());
    assert_eq!(after_error.len(), error_before.len() + 1);
    assert!(
        matches!(after_error.last(), Some(AgentMessage::Error { message }) if message.contains("offline inference failure"))
    );
    let conversation = messages(command(
        app,
        "outer-context",
        "GET msg.conversation",
        vec![],
    ));
    assert_eq!(conversation.last(), after_error.last());
    assert_eq!(
        &after_error[before.len()..before.len() + 3],
        &[outer_assistant, outer_error, outer_tool]
    );

    let calls = (0..2)
        .map(|index| ToolCall {
            id: format!("per-result-call-{index}"),
            tool_name: "list_dir".into(),
            arguments: "{}".into(),
        })
        .collect::<Vec<_>>();
    let batch = assistant("external two-call assistant", calls.clone());
    inject_messages(app, "per-result-batch", &[batch.clone()]);
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    tick_until(app, "two mocked external calls", |world| {
        for request in world.event_reader::<ToolExecuteRequest>() {
            if request.agent != entity || !seen.insert(request.call.id.clone()) {
                continue;
            }
            assert!(calls.contains(&request.call));
            assert!(request.turn.is_none());
            let message = AgentMessage::ToolCall {
                resource_id: Some(request.resource.resource_id.clone()),
                tool_call_id: request.call.id.clone(),
                tool_name: request.call.tool_name.clone(),
                content: format!("per-result mock {}", request.call.id),
                failed: false,
            };
            request
                .reply
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(Ok(ToolExecuteResponse {
                    message: message.clone(),
                }))
                .unwrap();
            results.push(message);
        }
        (seen.len() == 2).then_some(())
    });
    let first = inference(app, entity);
    assert!(first.turn.is_none());
    assert_eq!(
        results
            .iter()
            .filter(|result| first.messages.contains(result))
            .count(),
        1,
        "the first tool result must trigger inference before the second result is consumed"
    );
    let first_answer = assistant("first per-result answer", vec![]);
    respond(&first, first_answer.clone(), StopReason::Completed);
    let second = inference(app, entity);
    assert!(second.turn.is_none());
    assert!(
        second.messages.contains(&first_answer),
        "inference result must be enqueued at the head before the remaining tool result"
    );
    for result in &results {
        assert!(second.messages.contains(result));
    }
    let second_answer = assistant("second per-result answer", vec![]);
    respond(&second, second_answer.clone(), StopReason::Completed);
    turn_done(app, entity);
    let history = history_messages(app, entity);
    assert_eq!(
        history.iter().filter(|message| **message == batch).count(),
        1
    );
    for message in results.iter().chain([&first_answer, &second_answer]) {
        assert_eq!(history.iter().filter(|item| *item == message).count(), 1);
    }
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
        let pending = agent
            .block
            .get(&BlockPath {
                block_id: "entry".into(),
                inner_id: "pending".into(),
            })
            .ok()?;
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
fn exercise(images: &Path, image: &str, reviewer: bool) {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app(images);
    let (workspace, entity) = start(&mut app, temp.path(), image);
    let tools = if reviewer {
        vec!["list-directory"]
    } else {
        vec![
            "list-directory",
            "read-file",
            "write-file",
            "edit",
            "grep",
            "glob",
            "bash",
        ]
    };
    let expected_tools = tools
        .iter()
        .map(|name| id(&format!("tool:local/{name}:latest")))
        .collect::<BTreeSet<_>>();
    let tool_set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(entity)
        .unwrap();
    for resource in &expected_tools {
        assert!(
            tool_set.get(resource).is_some(),
            "missing actual tool {resource}"
        );
    }
    let prompt_set = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(entity)
        .unwrap();
    for name in [
        if reviewer {
            "reviewer-soul"
        } else {
            "coder-soul"
        },
        "compact",
    ] {
        assert!(prompt_set
            .get(&id(&format!("prompt:local/{name}:latest")))
            .is_some());
    }
    let skills = app
        .world()
        .get_component::<ResourceSet<SkillResource>>(entity)
        .unwrap();
    assert!(skills.get(&id("skill:local/code-review:latest")).is_some());
    if reviewer {
        assert!(skills
            .get(&id("skill:local/project-context:latest"))
            .is_some());
    }
    let sandboxes = app
        .world()
        .get_component::<ResourceSet<SandboxResource>>(entity)
        .unwrap();
    for name in ["workspace-write", "read-only"] {
        assert!(sandboxes
            .get(&id(&format!("sandbox:local/{name}:latest")))
            .is_some());
    }
    let selected_sandbox = id(if reviewer {
        "sandbox:local/read-only:latest"
    } else {
        "sandbox:local/workspace-write:latest"
    });
    let selected_policy = sandboxes.get(&selected_sandbox).unwrap().policy.clone();
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let sandbox = agent
        .sandbox
        .as_ref()
        .expect("sandbox selected by typed snapshot");
    assert_eq!(sandbox.resource_id, selected_sandbox);
    assert_eq!(sandbox.policy, selected_policy);
    let soul_name = if reviewer {
        "reviewer-soul"
    } else {
        "coder-soul"
    };
    let soul_content = fs::read_to_string(
        global_home().join(format!("prompts/local/{soul_name}/latest/content.md")),
    )
    .unwrap();
    let compact_content =
        fs::read_to_string(global_home().join("prompts/local/compact/latest/content.md")).unwrap();
    assert!(!soul_content.trim().is_empty());
    assert!(!compact_content.trim().is_empty());
    let soul = vec![AgentMessage::System {
        content: soul_content,
    }];
    let compact = vec![AgentMessage::User {
        content: compact_content,
    }];
    assert_eq!(
        messages(command(&mut app, "soul-alias", "GET soul", vec![])),
        soul
    );
    assert_eq!(
        messages(command(&mut app, "compact-alias", "GET compact", vec![])),
        compact
    );
    assert_eq!(
        messages(command(
            &mut app,
            "instructions",
            "GET msg.instructions",
            vec![]
        )),
        soul
    );
    assert_eq!(
        messages(command(
            &mut app,
            "compact-prompt",
            "GET msg.compact_prompt",
            vec![]
        )),
        compact
    );
    let MclValue::Array(BlockInner::Tool(visible)) =
        command(&mut app, "visible", "GET setting.visible", vec![])
    else {
        panic!("expected typed callable snapshots");
    };
    let mut expected_visible = expected_tools.clone();
    expected_visible.insert(id("skill:local/code-review:latest"));
    if reviewer {
        expected_visible.insert(id("skill:local/project-context:latest"));
    }
    assert_eq!(
        visible
            .iter()
            .map(|value| value.resource_id.clone())
            .collect::<BTreeSet<_>>(),
        expected_visible
    );
    assert_eq!(visible.len(), expected_visible.len());
    for value in &visible {
        match value.definition.as_ref() {
            CallableDefinitionSnapshot::Tool(definition) => assert_eq!(
                Some(definition),
                app.world()
                    .get_component::<ResourceSet<ToolResource>>(entity)
                    .unwrap()
                    .get(&value.resource_id),
            ),
            CallableDefinitionSnapshot::Skill(definition) => assert_eq!(
                Some(definition),
                app.world()
                    .get_component::<ResourceSet<SkillResource>>(entity)
                    .unwrap()
                    .get(&value.resource_id),
            ),
            CallableDefinitionSnapshot::Hook(_) => panic!("unexpected hook in home image"),
        }
    }
    assert_eq!(
        visible
            .iter()
            .find(|value| value.resource_id == id("tool:local/list-directory:latest"))
            .unwrap()
            .alias,
        "list_dir"
    );
    let MclValue::Array(BlockInner::Sandbox(options)) = command(
        &mut app,
        "sandbox-options",
        "GET setting.sandbox_options",
        vec![],
    ) else {
        panic!("expected typed sandbox snapshots");
    };
    assert_eq!(options.len(), 2);
    assert_eq!(
        options
            .iter()
            .map(|value| value.resource_id.clone())
            .collect::<BTreeSet<_>>(),
        [
            id("sandbox:local/workspace-write:latest"),
            id("sandbox:local/read-only:latest")
        ]
        .into_iter()
        .collect()
    );
    for value in &options {
        assert_eq!(
            value.policy,
            app.world()
                .get_component::<ResourceSet<SandboxResource>>(entity)
                .unwrap()
                .get(&value.resource_id)
                .unwrap()
                .policy
        );
    }
    assert_eq!(
        command(
            &mut app,
            "sandbox-activated",
            "GET setting.sandbox_activated",
            vec![]
        ),
        MclValue::Array(BlockInner::Sandbox(
            options
                .into_iter()
                .filter(|value| value.resource_id == selected_sandbox)
                .collect()
        ))
    );
    inject(&mut app, "plain", "offline first user");
    let first = inference(&mut app, entity);
    let first_turn = first.turn.clone().expect("turn active");
    assert_eq!(
        first
            .tools
            .iter()
            .filter(|resource| resource.resource_id.resource_type() == "tool")
            .map(|resource| resource.resource_id.clone())
            .collect::<BTreeSet<_>>(),
        expected_tools
    );
    assert_eq!(first.tools, visible);
    assert_eq!(
        first.messages,
        [
            soul.clone(),
            vec![AgentMessage::User {
                content: "offline first user".into()
            }]
        ]
        .concat()
    );
    assert_eq!(first.messages.iter().filter(|message| matches!(message, AgentMessage::User { content } if content == "offline first user")).count(), 1);
    let first_answer = assistant("offline first answer", vec![]);
    respond(&first, first_answer.clone(), StopReason::Completed);
    turn_done(&mut app, entity);
    inject(&mut app, "tools", "offline tool user");
    let second = inference(&mut app, entity);
    let second_turn = second.turn.clone().expect("second turn active");
    assert_ne!(first_turn, second_turn);
    assert_eq!(
        second
            .messages
            .iter()
            .filter(|message| **message == first_answer)
            .count(),
        1
    );
    assert_eq!(second.tools, first.tools);
    let calls = (0..1)
        .map(|index| ToolCall {
            id: format!("offline-call-{index}"),
            tool_name: "list_dir".into(),
            arguments: "{}".into(),
        })
        .collect::<Vec<_>>();
    let batch_answer = assistant("offline batch", calls.clone());
    respond(&second, batch_answer.clone(), StopReason::ToolCalls);
    let mut tool_messages = Vec::new();
    let mut seen = BTreeSet::new();
    tick_until(&mut app, "mock tool batch", |world| {
        for request in world.event_reader::<ToolExecuteRequest>() {
            if request.agent != entity || !seen.insert(request.call.id.clone()) {
                continue;
            }
            assert_eq!(
                request.resource.resource_id,
                id("tool:local/list-directory:latest")
            );
            assert_eq!(
                Some(&request.resource),
                visible
                    .iter()
                    .find(|value| value.resource_id == request.resource.resource_id)
            );
            assert_eq!(request.turn.as_deref(), Some(second_turn.as_str()));
            assert!(calls.contains(&request.call));
            assert!(!request.cancel.is_cancelled());
            let message = AgentMessage::ToolCall {
                resource_id: Some(request.resource.resource_id.clone()),
                tool_call_id: request.call.id.clone(),
                tool_name: request.call.tool_name.clone(),
                content: format!("mock result{}", request.call.id),
                failed: false,
            };
            request
                .reply
                .lock()
                .unwrap()
                .take()
                .expect("tool reply consumed once")
                .send(Ok(ToolExecuteResponse {
                    message: message.clone(),
                }))
                .unwrap();
            tool_messages.push(message);
        }
        (seen.len() == calls.len()).then_some(())
    });
    let third = inference(&mut app, entity);
    assert_eq!(third.turn, second.turn);
    assert_eq!(third.tools, first.tools);
    assert_eq!(
        third
            .messages
            .iter()
            .filter(|message| **message == batch_answer)
            .count(),
        1
    );
    for message in &tool_messages {
        assert_eq!(
            third
                .messages
                .iter()
                .filter(|item| *item == message)
                .count(),
            1
        );
    }
    let final_answer = assistant("offline final answer", vec![]);
    respond(&third, final_answer.clone(), StopReason::Completed);
    turn_done(&mut app, entity);
    let context_only = assistant("offline context-only injection", vec![]);
    let context_user = AgentMessage::User {
        content: "context-only user must not start inference".into(),
    };
    let context_error = AgentMessage::Error {
        message: "context-only error must not dispatch finish".into(),
    };
    let context_container = AgentMessage::Inject {
        messages: vec![
            context_only.clone(),
            AgentMessage::Inject {
                messages: vec![context_user.clone(), context_error.clone()],
            },
        ],
    };
    command(
        &mut app,
        "inject-context-only",
        "INJECT ? TO entry.pending",
        vec![serde_json::json!([context_container.clone()])],
    );
    command(&mut app, "wake-context-only", "EMIT EFFECT wake", vec![]);
    tick_until(&mut app, "context-only injection recorded", |world| {
        let agent = world.get_component::<Agent>(entity)?;
        let BlockInner::Message(conversation) = agent
            .block
            .get(&BlockPath {
                block_id: "msg".into(),
                inner_id: "conversation".into(),
            })
            .ok()?
        else {
            return None;
        };
        (conversation.contains(&context_only)
            && agent.history.list().unwrap().iter().any(|record| {
                record.kind == "message"
                    && record.payload == serde_json::to_value(&context_container).unwrap()
            }))
        .then_some(())
    });
    turn_done(&mut app, entity);
    assert!(app
        .world()
        .event_reader::<InferenceRequest>()
        .into_iter()
        .all(|request| request.agent != entity));
    let context = messages(command(&mut app, "context", "GET msg.conversation", vec![]));
    let expected = vec![
        AgentMessage::User {
            content: "offline first user".into(),
        },
        first_answer,
        AgentMessage::User {
            content: "offline tool user".into(),
        },
        batch_answer,
        tool_messages.remove(0),
        final_answer,
        context_only,
        context_user,
        context_error,
    ];
    for message in &expected {
        assert_eq!(
            context.iter().filter(|item| *item == message).count(),
            1,
            "missing/duplicated {message:?}; context={context:?}"
        );
    }
    assert_eq!(context, expected);
    let history = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap();
    let stored_messages = history
        .iter()
        .filter(|record| record.kind == "message")
        .map(|record| serde_json::from_value::<AgentMessage>(record.payload.clone()).unwrap())
        .collect::<Vec<_>>();
    let mut expected_history = expected[..expected.len() - 3].to_vec();
    expected_history.push(context_container);
    assert_eq!(
        stored_messages, expected_history,
        "history must preserve the original nested inject container, not flattened context leaves"
    );
    assert_eq!(
        history
            .iter()
            .map(|record| &record.key)
            .collect::<BTreeSet<_>>()
            .len(),
        history.len()
    );
    command(
        &mut app,
        "large-context",
        "INJECT ? TO msg.conversation[-1]",
        vec![serde_json::json!([
            {"type": "user", "content": "x".repeat(120001)}
        ])],
    );
    inject(&mut app, "compact", "offline post-compaction user");
    let compression = inference(&mut app, entity);
    assert!(compression.tools.is_empty());
    assert!(compression.turn.is_none());
    assert_eq!(&compression.messages[..soul.len()], soul.as_slice());
    assert_eq!(
        &compression.messages[compression.messages.len() - compact.len()..],
        compact.as_slice()
    );
    for message in &expected {
        assert_eq!(
            compression
                .messages
                .iter()
                .filter(|item| *item == message)
                .count(),
            1
        );
    }
    assert!(compression.messages.iter().any(
        |message| matches!(message, AgentMessage::User { content } if content.len() == 120001)
    ));
    let summary = assistant("offline concise checkpoint", vec![]);
    respond(&compression, summary.clone(), StopReason::Completed);
    let after_compression = inference(&mut app, entity);
    assert!(after_compression.turn.is_some());
    assert_eq!(after_compression.tools, first.tools);
    let checkpoint = AgentMessage::User {
        content: "Conversation checkpoint:\noffline concise checkpoint".into(),
    };
    let new_user = AgentMessage::User {
        content: "offline post-compaction user".into(),
    };
    assert!(
        after_compression.messages.contains(&checkpoint),
        "missing checkpoint after completed compaction: {:?}",
        after_compression.messages
    );
    assert_eq!(
        after_compression.messages,
        [soul, vec![checkpoint.clone(), new_user.clone()]].concat()
    );
    for message in &expected {
        assert!(
            !after_compression.messages.contains(message),
            "old conversation survived compaction"
        );
    }
    assert!(!after_compression.messages.contains(&summary));
    assert!(after_compression.messages.contains(&new_user));
    assert!(!after_compression.messages.iter().any(
        |message| matches!(message, AgentMessage::User { content } if content.len() == 120001)
    ));
    let compact_final = assistant("offline post-compaction answer", vec![]);
    respond(
        &after_compression,
        compact_final.clone(),
        StopReason::Completed,
    );
    turn_done(&mut app, entity);
    assert_eq!(
        messages(command(
            &mut app,
            "compacted-context",
            "GET msg.conversation",
            vec![]
        )),
        vec![checkpoint, new_user.clone(), compact_final.clone()]
    );
    let complete_history = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap();
    assert_eq!(&complete_history[..history.len()], history.as_slice());
    let appended_messages = complete_history[history.len()..]
        .iter()
        .filter(|record| record.kind == "message")
        .map(|record| serde_json::from_value::<AgentMessage>(record.payload.clone()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(appended_messages, vec![summary, new_user, compact_final]);
    exercise_entry_dispatch(&mut app, entity);
    let history = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .history
        .list()
        .unwrap();
    stop(&mut app, workspace, entity);
    let persisted = SqliteHistoryStore::open(temp.path().join("history.sqlite"))
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(&persisted[..history.len()], history.as_slice());
    assert!(
        persisted[history.len()..]
            .iter()
            .all(|record| { record.kind == "mcl" && record.content == "GET entry.pending[0]" }),
        "only the driver's final idle read may race the history snapshot"
    );
}
#[test]
#[ignore = "requires migrated home image packages"]
fn actual_home_coder_driver_offline_lifecycle() {
    exercise(
        &global_home().join("agent-images"),
        "image:local/coder:latest",
        false,
    );
}
#[test]
#[ignore = "requires migrated home image packages"]
fn actual_home_reviewer_driver_offline_lifecycle() {
    exercise(
        &global_home().join("agent-images"),
        "image:local/reviewer:latest",
        true,
    );
}
#[test]
#[ignore = "requires migrated home image packages"]
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
    exercise(temp.path(), "image:local/coder:latest", false);
}
