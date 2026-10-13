use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{
    Agent, AgentInfo, AgentState, ImportedValue, WorldAgentExt,
};
use margatroid_types::block_types::AgentBlock;
use margatroid_types::execution_components::{AgentExecutionScope, ExecutionActivityKind};
use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use margatroid_types::message_types::{AgentMessage, ToolCall};
use margatroid_types::resource_components::{
    AgentImageModelConfig, CallableDefinitionSnapshot, CallableResource, HookResource,
    SandboxResource, SandboxValue, SkillResource, ToolResource,
};
use resource_id_plugin::{ResourceID, ResourceSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tool_plugin::{
    ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse, ToolExecutionLimits,
    ToolPlugin,
};

struct History;
impl AgentHistoryStore for History {
    fn append_batch(&self, _: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        panic!("ToolPlugin must not record history")
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        Ok(vec![])
    }
}
fn id(kind: &str, name: &str) -> ResourceID {
    ResourceID::parse(&format!("{kind}:test/{name}:latest")).unwrap()
}
fn fixture(limits: ToolExecutionLimits) -> (App, Entity, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(resource_id_plugin::ResourcePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(
            ToolPlugin::default()
                .with_runner_path(env!("CARGO_BIN_EXE_tool_runner"))
                .with_limits(limits),
        );
    use resource_id_plugin::WorldResourceExt;
    app.world_mut().register_library::<Agent>("agent").unwrap();
    let entity = app.world_mut().spawn();
    let agent_id = id("agent", "agent");
    app.world_mut().insert_component(
        entity,
        Agent {
            resourceid: agent_id,
            workspace: id("workspace", "workspace"),
            sandbox: None,
            info: AgentInfo {
                model: AgentImageModelConfig {
                    model: "test".into(),
                    temperature: None,
                    max_output_tokens: None,
                    top_p: None,
                    stop: vec![],
                },
                project_root: root.path().into(),
                home_root: root.path().into(),
                image: id("agent_image", "image"),
                image_dependencies: Arc::from([]),
            },
            lua: Default::default(),
            state: AgentState::Running,
            history: AgentHistoryHandle::new(History),
            block: AgentBlock::default(),
            import_bindings: Default::default(),
            mcl: Default::default(),
            execution: AgentExecutionScope::new(),
            turn: Some("live-turn".into()),
        },
    );
    (app, entity, root)
}
fn add_tool(
    app: &mut App,
    entity: Entity,
    root: &std::path::Path,
    script: &str,
    schema: serde_json::Value,
) -> ResourceID {
    let resource = id("tool", "echo");
    let path = root.join("main.lua");
    std::fs::write(&path, script).unwrap();
    let mut set = ResourceSet::new();
    set.insert(
        resource.clone(),
        ToolResource {
            alias: "keep-alias".into(),
            description: "test".into(),
            input_schema: schema,
            script_path: path,
        },
    );
    assert!(app.world_mut().insert_component(entity, set));
    bind(app, entity, &resource);
    resource
}
fn bind(app: &mut App, entity: Entity, resource: &ResourceID) {
    let definition = match resource.resource_type() {
        "tool" => CallableDefinitionSnapshot::Tool(
            app.world()
                .get_component::<ResourceSet<ToolResource>>(entity)
                .unwrap()
                .get(resource)
                .unwrap()
                .clone(),
        ),
        "skill" => CallableDefinitionSnapshot::Skill(
            app.world()
                .get_component::<ResourceSet<SkillResource>>(entity)
                .unwrap()
                .get(resource)
                .unwrap()
                .clone(),
        ),
        "hook" => CallableDefinitionSnapshot::Hook(
            app.world()
                .get_component::<ResourceSet<HookResource>>(entity)
                .unwrap()
                .get(resource)
                .unwrap()
                .clone(),
        ),
        _ => panic!("unsupported fixture"),
    };
    let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
    let mut dependencies = agent.info.image_dependencies.to_vec();
    dependencies.push(resource.clone());
    agent.info.image_dependencies = dependencies.into();
    let id = agent.resourceid.clone();
    app.world_mut()
        .agent_bind_import(
            &id,
            "keep-alias".into(),
            ImportedValue::Tool(CallableResource {
                resource_id: resource.clone(),
                alias: "keep-alias".into(),
                definition: Arc::new(definition),
            }),
        )
        .unwrap();
}
fn submit(
    app: &App,
    entity: Entity,
    resource: ResourceID,
    arguments: &str,
) -> (
    ToolExecuteRequest,
    oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
) {
    let (tx, rx) = oneshot::channel();
    let definition = match resource.resource_type() {
        "tool" => CallableDefinitionSnapshot::Tool(
            app.world()
                .get_component::<ResourceSet<ToolResource>>(entity)
                .unwrap()
                .get(&resource)
                .unwrap()
                .clone(),
        ),
        "skill" => CallableDefinitionSnapshot::Skill(
            app.world()
                .get_component::<ResourceSet<SkillResource>>(entity)
                .unwrap()
                .get(&resource)
                .unwrap()
                .clone(),
        ),
        "hook" => CallableDefinitionSnapshot::Hook(
            app.world()
                .get_component::<ResourceSet<HookResource>>(entity)
                .unwrap()
                .get(&resource)
                .unwrap()
                .clone(),
        ),
        _ => panic!("unsupported fixture"),
    };
    let request = ToolExecuteRequest {
        id: "request".into(),
        agent: entity,
        resource: CallableResource {
            resource_id: resource,
            alias: "keep-alias".into(),
            definition: Arc::new(definition),
        },
        call: ToolCall {
            id: "call".into(),
            tool_name: "keep-alias".into(),
            arguments: arguments.into(),
        },
        turn: Some("snapshot-turn".into()),
        cancel: tokio_util::sync::CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(tx))),
    };
    app.world().send_event(request.clone());
    (request, rx)
}
async fn response(
    app: &mut App,
    mut rx: oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
) -> Result<ToolExecuteResponse, ToolError> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.tick();
            match rx.try_recv() {
                Ok(result) => return result,
                Err(oneshot::error::TryRecvError::Closed) => panic!("reply lost"),
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn content(result: ToolExecuteResponse) -> String {
    match result.message {
        AgentMessage::ToolCall {
            tool_name,
            tool_call_id,
            resource_id,
            content,
            failed,
        } => {
            assert_eq!(tool_name, "keep-alias");
            assert_eq!(tool_call_id, "call");
            assert!(resource_id.is_some());
            assert!(!failed);
            content
        }
        _ => panic!("not tool call"),
    }
}
#[tokio::test]
async fn actual_runner_preserves_alias_context_and_raw_string() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(&mut app, entity, root.path(), "function execute(a,c) print('diagnostic'); return json.encode({value=a.value,turn=c.turn,call=c.call_id}) end", serde_json::json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}));
    let (_, rx) = submit(&app, entity, resource, "{\"value\":\"ok\"}");
    let result = content(response(&mut app, rx).await.unwrap());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result).unwrap(),
        serde_json::json!({"value":"ok","turn":"snapshot-turn","call":"call"})
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}
#[tokio::test]
async fn skill_empty_object_direct_body_and_invalid_arguments() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let path = root.path().join("SKILL.md");
    std::fs::write(&path, "# skill\nbody").unwrap();
    let resource = id("skill", "skill");
    let mut set = ResourceSet::new();
    set.insert(
        resource.clone(),
        SkillResource {
            alias: "keep-alias".into(),
            description: "skill".into(),
            content_path: path,
        },
    );
    assert!(app.world_mut().insert_component(entity, set));
    bind(&mut app, entity, &resource);
    let sandbox = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .register(ExecutionActivityKind::Sandbox)
        .unwrap();
    let (_, rx) = submit(&app, entity, resource.clone(), "{}");
    assert_eq!(
        content(response(&mut app, rx).await.unwrap()),
        "# skill\nbody"
    );
    sandbox.mark_completed().unwrap();
    let (_, rx) = submit(&app, entity, resource, "{\"extra\":true}");
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::InvalidArguments
    );
}
#[tokio::test]
async fn invalid_schema_and_hook_rejected_before_activity() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'bad' end",
        serde_json::json!({"type":123}),
    );
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::InvalidDefinition
    );
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let mut hooks = ResourceSet::new();
    hooks.insert(
        id("hook", "hook"),
        HookResource {
            alias: "keep-alias".into(),
            description: "hook".into(),
            input_schema: serde_json::json!({"type":"object"}),
            script_path: root.path().join("never.lua"),
        },
    );
    app.world_mut().insert_component(entity, hooks);
    bind(&mut app, entity, &id("hook", "hook"));
    let (_, rx) = submit(&app, entity, id("hook", "hook"), "{}");
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::UnsupportedHook
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}
#[tokio::test]
async fn parallel_large_shell_pipes_do_not_deadlock() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(&mut app, entity, root.path(), "function execute() local r=shell('head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2'); return tostring(#r.stdout)..':'..tostring(#r.stderr) end", serde_json::json!({"type":"object"}));
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(
        content(response(&mut app, rx).await.unwrap()),
        "100000:100000"
    );
}
#[tokio::test]
async fn timeout_and_logical_cancel_reap_processes_before_scope_release() {
    for explicit in [false, true] {
        let mut limits = ToolExecutionLimits::default();
        limits.execution_timeout = Duration::from_millis(200);
        limits.host_call_timeout = limits.execution_timeout;
        let (mut app, entity, root) = fixture(limits);
        let resource = add_tool(
            &mut app,
            entity,
            root.path(),
            "function execute() return shell('sleep 30 & wait').stdout end",
            serde_json::json!({"type":"object"}),
        );
        let (request, rx) = submit(&app, entity, resource, "{}");
        app.tick();
        app.tick();
        if explicit {
            tokio::time::sleep(Duration::from_millis(50)).await;
            request.cancel.cancel();
        }
        let err = response(&mut app, rx).await.unwrap_err();
        assert_eq!(
            err.kind,
            if explicit {
                ToolErrorKind::Cancelled
            } else {
                ToolErrorKind::Timeout
            }
        );
        assert_eq!(
            app.world()
                .get_component::<Agent>(entity)
                .unwrap()
                .execution
                .activity_count(),
            0
        );
    }
}
#[tokio::test]
async fn sandbox_backend_unavailable_fails_closed() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'must not execute' end",
        serde_json::json!({"type":"object"}),
    );
    let sandbox = id("sandbox", "policy");
    let mut set = ResourceSet::new();
    set.insert(
        sandbox.clone(),
        SandboxResource {
            alias: "sandbox".into(),
            policy: Arc::from("{}"),
        },
    );
    assert!(app.world_mut().insert_component(entity, set));
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .sandbox = Some(SandboxValue {
        resource_id: sandbox,
        policy: Arc::from("{}"),
    });
    let previous = std::env::var_os("MARGATROID_SANDBOX_BACKEND");
    std::env::set_var(
        "MARGATROID_SANDBOX_BACKEND",
        root.path().join("missing-backend"),
    );
    let (_, rx) = submit(&app, entity, resource, "{}");
    let result = response(&mut app, rx).await;
    if let Some(previous) = previous {
        std::env::set_var("MARGATROID_SANDBOX_BACKEND", previous);
    } else {
        std::env::remove_var("MARGATROID_SANDBOX_BACKEND");
    }
    assert_eq!(result.unwrap_err().kind, ToolErrorKind::RunnerStartFailed);
}
#[tokio::test]
async fn escaped_session_descendant_is_reaped_by_linux_subreaper() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(&mut app, entity, root.path(), "function execute() shell('setsid sleep 30 >/dev/null 2>&1 & echo $! > child-pid'); return 'done' end", serde_json::json!({"type":"object"}));
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(content(response(&mut app, rx).await.unwrap()), "done");
    let pid = std::fs::read_to_string(root.path().join("child-pid"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "escaped child survived cleanup"
    );
}
#[tokio::test]
async fn explicit_pty_has_tty_large_output_and_nonzero_status() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(&mut app, entity, root.path(), "function execute() local r=shell('test -t 0 && test -t 1 && test -t 2 || exit 99; head -c 100000 /dev/zero; exit 7',{pty=true}); return tostring(#r.stdout)..':'..tostring(r.code)..':'..r.stderr end", serde_json::json!({"type":"object"}));
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(content(response(&mut app, rx).await.unwrap()), "100000:7:");
}
#[tokio::test]
async fn pty_cancel_closes_terminal_and_reaps_session() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(&mut app, entity, root.path(), "function execute() return shell('sleep 30 & echo $! > pty-child; wait',{pty=true}).stdout end", serde_json::json!({"type":"object"}));
    let (request, rx) = submit(&app, entity, resource, "{}");
    app.tick();
    app.tick();
    tokio::time::sleep(Duration::from_millis(80)).await;
    request.cancel.cancel();
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::Cancelled
    );
    let pid = std::fs::read_to_string(root.path().join("pty-child"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}
#[tokio::test]
async fn duplicate_events_claim_one_execution() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() shell('echo once >> count'); return 'ok' end",
        serde_json::json!({"type":"object"}),
    );
    let (request, rx) = submit(&app, entity, resource, "{}");
    app.world().send_event(request);
    assert_eq!(content(response(&mut app, rx).await.unwrap()), "ok");
    assert_eq!(
        std::fs::read_to_string(root.path().join("count")).unwrap(),
        "once\n"
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}
#[tokio::test]
async fn invalid_protocol_retains_cleanup_failed_registration() {
    use std::os::unix::fs::PermissionsExt;
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let runner = root.path().join("fake-runner");
    std::fs::write(&runner, "#!/bin/sh\ncat >/dev/null\necho not-protocol\n").unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.world_mut()
        .get_resource_mut::<ToolPlugin>()
        .unwrap()
        .runner_path = runner;
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'unused' end",
        serde_json::json!({"type":"object"}),
    );
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::CleanupFailed
    );
    let scope = &app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution;
    assert_eq!(scope.activity_count(), 1);
    assert!(scope.wait_all().await.is_err());
}
#[tokio::test]
async fn receiver_closed_cancels_and_reaps_without_false_completion() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return shell('sleep 30').stdout end",
        serde_json::json!({"type":"object"}),
    );
    let (_, rx) = submit(&app, entity, resource, "{}");
    app.tick();
    app.tick();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(rx);
    let scope = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .clone();
    tokio::time::timeout(Duration::from_secs(5), scope.wait_all())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scope.activity_count(), 0);
}
#[tokio::test]
async fn queued_submission_drop_rolls_back_and_replies() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'unused' end",
        serde_json::json!({"type":"object"}),
    );
    let (_, rx) = submit(&app, entity, resource, "{}");
    app.tick();
    let scope = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .clone();
    assert_eq!(scope.activity_count(), 1);
    drop(app);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        ToolErrorKind::SubmitFailed
    );
    assert_eq!(scope.activity_count(), 0);
}
#[tokio::test]
async fn output_limits_and_handoff_are_explicit() {
    for (script, kind) in [
        (
            "function execute() return string.rep('x',5000) end",
            ToolErrorKind::OutputTooLarge,
        ),
        (
            "function execute() handoff({}) return 'bad' end",
            ToolErrorKind::UnsupportedHandoff,
        ),
    ] {
        let mut limits = ToolExecutionLimits::default();
        limits.output_bytes = 1024;
        let (mut app, entity, root) = fixture(limits);
        let resource = add_tool(
            &mut app,
            entity,
            root.path(),
            script,
            serde_json::json!({"type":"object"}),
        );
        let (_, rx) = submit(&app, entity, resource, "{}");
        assert_eq!(response(&mut app, rx).await.unwrap_err().kind, kind);
    }
}

#[tokio::test]
async fn selected_metadata_survives_collection_reload() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'selected' end",
        serde_json::json!({"type":"object"}),
    );
    let (_, rx) = submit(&app, entity, resource.clone(), "{}");
    let replacement = root.path().join("new.lua");
    std::fs::write(&replacement, "function execute() return 'latest' end").unwrap();
    let current = app
        .world_mut()
        .get_component_mut::<ResourceSet<ToolResource>>(entity)
        .unwrap()
        .get_mut(&resource)
        .unwrap();
    current.script_path = replacement;
    current.alias = "renamed".into();
    current.input_schema = serde_json::json!({"type":"object","required":["latest"]});
    assert_eq!(content(response(&mut app, rx).await.unwrap()), "selected");
}

#[tokio::test]
async fn loaded_but_unpermitted_resource_is_rejected_without_activity() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'forbidden' end",
        serde_json::json!({"type":"object"}),
    );
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .info
        .image_dependencies = Arc::from([]);
    let (_, rx) = submit(&app, entity, resource, "{}");
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::ResourceMissing
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}

#[tokio::test]
async fn selected_alias_must_match_call_name() {
    let (mut app, entity, root) = fixture(ToolExecutionLimits::default());
    let resource = add_tool(
        &mut app,
        entity,
        root.path(),
        "function execute() return 'forbidden' end",
        serde_json::json!({"type":"object"}),
    );
    let (request, rx) = submit(&app, entity, resource, "{}");
    drop(rx);
    let (tx, rx) = oneshot::channel();
    let mut mismatched = request;
    mismatched.call.tool_name = "another-alias".into();
    mismatched.reply = Arc::new(Mutex::new(Some(tx)));
    app.world().send_event(mismatched);
    assert_eq!(
        response(&mut app, rx).await.unwrap_err().kind,
        ToolErrorKind::InvalidRequest
    );
}
