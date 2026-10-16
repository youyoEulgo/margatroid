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
use margatroid_types::mcl_events::{MclCommand, MclCommandResult, MclInstruction, MclValue};
use margatroid_types::workspace_components::{
    Workspace, WorkspaceAgentDefinition, WorkspaceDefinition,
};
use margatroid_types::workspace_events::{
    StartWorkspace, StartWorkspaceResult, StopWorkspace, StopWorkspaceResult,
};
use mcl_plugin::MclPlugin;
use memory_plugin::MemoryPlugin;
use resource_id_plugin::{ResourceID, ResourcePlugin};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use workspace_plugin::WorkspacePlugin;

const DRIVER: &str = r#"
local function command(text, arguments)
    if arguments then return mcl(agent_info.resource_id, text, arguments) end
    return mcl(agent_info.resource_id, text)
end
command('CREATE TEMP BLOCK entry (pending MESSAGE)')
command('CREATE BLOCK session (processed MESSAGE)')
command('CREATE BLOCK resources (tools TOOL)')
command('EMIT EFFECT ready')
while true do
    local response = command('GET entry.pending')
    local messages = response.Array.Message
    if #messages > 0 then
        command('EMIT EFFECT start_turn')
        command('INJECT entry.pending TO session.processed[-1]')
        command('INJECT [] TO entry.pending')
        command('EMIT EFFECT finish_turn')
    else
        wait()
    end
end
"#;
fn id(value: &str) -> ResourceID {
    ResourceID::parse(value).unwrap()
}
fn driver(root: &Path, name: &str, driver: &str, dependencies: &[&str]) {
    let directory = root.join("drivers/local").join(name).join("latest");
    fs::create_dir_all(&directory).unwrap();
    let mut manifest = "schema_version = 1\n[inference]\nmodel = \"offline-test\"\n".to_owned();
    for dependency in dependencies {
        manifest.push_str(&format!("\n[[dependencies]]\nid = {dependency:?}\n"));
    }
    fs::write(directory.join("driver.toml"), manifest).unwrap();
    fs::write(directory.join("base.lua"), driver).unwrap();
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.world_mut().insert_resource(
        margatroid_types::model_components::ModelRuntimeCatalog::new([
            margatroid_types::model_components::ModelRuntimeInfo {
                id: "offline-test".into(),
                model: "offline-test".into(),
                api_type: "chat_completions".into(),
                provider: None,
                context_window_tokens: 128000,
                thinking: None,
                reasoning_effort: None,
            },
        ]),
    );
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(tool_resource_plugin::ToolResourcePlugin::open(root).unwrap())
        .add_plugin(prompt_resource_plugin::PromptResourcePlugin::open(root).unwrap())
        .add_plugin(sandbox_resource_plugin::SandboxResourcePlugin::open(root).unwrap())
        .add_plugin(AgentDriverLoaderPlugin::open(root.join("drivers")).unwrap())
        .add_plugin(MemoryPlugin::new())
        .add_plugin(MclPlugin::default())
        .add_plugin(LuaRuntimePlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(WorkspacePlugin::default());
    app
}
fn definition(root: &Path, drivers: &[&str]) -> WorkspaceDefinition {
    WorkspaceDefinition {
        id: id("workspace:local/lifecycle"),
        name: "lifecycle".into(),
        manager: "agent-0".into(),
        project_root: root.join("project"),
        agents: drivers
            .iter()
            .enumerate()
            .map(|(index, driver)| WorkspaceAgentDefinition {
                id: id(&format!("agent:local/agent-{index}")),
                name: format!("agent-{index}"),
                driver: id(&format!("driver:local/{driver}")),
                home_root: root.join(format!("home-{index}")),
                history_path: root.join(format!("history-{index}.sqlite")),
            })
            .collect(),
    }
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
        cancel: agent.execution.child_cancel(),
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
    let agents = world
        .query_with::<Agent>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            world.get_component::<Agent>(entity).map(|agent| {
                format!(
                    "{entity:?}{} {:?} activities={} queue={} pending={:?} drain={} wake={:?}",
                    agent.resourceid,
                    agent.state,
                    agent.execution.activity_count(),
                    agent.mcl.commands.len(),
                    agent.mcl.pending,
                    agent.mcl.drained.is_drained(),
                    agent.lua.wake
                )
            })
        })
        .collect::<Vec<_>>();
    let workspaces = world
        .query_with::<Workspace>()
        .result()
        .into_iter()
        .filter_map(|entity| {
            world
                .get_component::<Workspace>(entity)
                .map(|workspace| format!("{entity:?} {:?}", workspace.state()))
        })
        .collect::<Vec<_>>();
    format!(
        "agents={agents:?}; workspaces={workspaces:?}; normal_events={} pending_events={}",
        world.event_snapshot().normal_event_count,
        world.event_snapshot().pending_event_count
    )
}
fn tick_until<T>(app: &mut App, label: &str, mut inspect: impl FnMut(&World) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if let Some(value) = inspect(app.world()) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{label} timed out:{}",
            diagnostics(app.world())
        );
        std::thread::yield_now();
    }
}
fn start(
    app: &mut App,
    definition: WorkspaceDefinition,
) -> Result<Entity, margatroid_types::workspace_components::WorkspaceError> {
    app.world().send_event(StartWorkspace {
        id: "start-workspace".into(),
        definition,
    });
    tick_until(app, "workspace startup", |world| {
        world
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "start-workspace")
            .map(|event| event.result.clone())
    })
}
fn command(
    app: &mut App,
    target: &ResourceID,
    request: &str,
    text: &str,
    arguments: Vec<serde_json::Value>,
) -> MclCommandResult {
    app.world().send_event(MclCommand {
        id: request.into(),
        source: id("client:webui/test:1"),
        target: target.clone(),
        command: MclInstruction {
            text: text.into(),
            arguments,
        },
    });
    tick_until(app, "client MCL result", |world| {
        world
            .event_reader::<MclCommandResult>()
            .into_iter()
            .find(|event| event.id == request)
            .cloned()
    })
}
fn stop(app: &mut App, workspace: Entity) {
    let members = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .map(|(_, entity)| {
            let agent = app.world().get_component::<Agent>(entity).unwrap();
            (entity, agent.execution.clone(), agent.mcl.drained.clone())
        })
        .collect::<Vec<_>>();
    let close = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .execution_close();
    app.world().send_event(StopWorkspace {
        id: "stop-workspace".into(),
        workspace,
    });
    let result = tick_until(app, "workspace stop", |world| {
        world
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "stop-workspace")
            .map(|event| event.result.clone())
    });
    assert_eq!(result.unwrap(), workspace);
    assert!(!app.world().is_alive(workspace));
    for (entity, scope, drain) in members {
        assert!(!app.world().is_alive(entity));
        assert_eq!(scope.activity_count(), 0);
        assert!(scope.is_closed());
        assert!(drain.is_drained());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), close.close())
            .await
            .unwrap()
            .unwrap();
    });
}
#[test]
fn two_agents_in_one_workspace_drive_arrays_wake_and_join_cleanly() {
    let root = tempfile::tempdir().unwrap();
    driver(root.path(), "base", DRIVER, &[]);
    let mut app = app(root.path());
    let workspace = start(&mut app, definition(root.path(), &["base", "base"])).unwrap();
    let members = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .map(|(id, entity)| (id.clone(), entity))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(members.len(), 2);
    for (agent_id, entity) in &members {
        let agent = app.world().get_component::<Agent>(*entity).unwrap();
        assert!(matches!(agent.state, AgentState::Idle));
        assert_eq!(agent.lua.instance.as_ref().unwrap().owner, *entity);
        assert!(agent.lua.ready.is_none());
        assert_eq!(
            command(
                &mut app,
                agent_id,
                &format!("forged-ready-{entity:?}"),
                "EMIT EFFECT ready",
                vec![]
            )
            .result,
            Err(margatroid_types::mcl_events::MclError::InvalidRequest)
        );
    }
    for agent_id in members.keys() {
        assert_eq!(
            command(
                &mut app,
                agent_id,
                &format!("reopen-{agent_id}"),
                "CREATE BLOCK session (processed MESSAGE)",
                vec![]
            )
            .result
            .unwrap(),
            MclValue::BlockOpened {
                block_id: "session".into(),
                created: false,
                temporary: false
            }
        );
        let agent = app
            .world()
            .get_component::<Agent>(members[agent_id])
            .unwrap();
        assert!(agent
            .history
            .list()
            .unwrap()
            .iter()
            .all(|record| record.kind != "message"));
    }
    let first_id = id("agent:local/agent-0");
    let first = members[&first_id];
    let second = members[&id("agent:local/agent-1")];
    let payload = serde_json::json!([{"type":"user", "content":"wake first agent", "alias":null}]);
    assert!(command(
        &mut app,
        &first_id,
        "inject-client",
        "INJECT ? TO entry.pending",
        vec![payload]
    )
    .result
    .is_ok());
    assert!(matches!(
        command(
            &mut app,
            &first_id,
            "get-client",
            "GET entry.pending",
            vec![]
        )
        .result,
        Ok(MclValue::Array(BlockInner::Message(_)))
    ));
    command(
        &mut app,
        &first_id,
        "wake-first",
        "EMIT EFFECT wake",
        vec![],
    )
    .result
    .unwrap();
    tick_until(&mut app, "driver processing after wake", |world| {
        let agent = world.get_component::<Agent>(first)?;
        let processed = stored(agent, "session", "processed");
        let pending = stored(agent, "entry", "pending");
        (processed.len() == 1 && pending.is_empty() && agent.turn.is_none()).then_some(())
    });
    assert!(app
        .world()
        .get_component::<Agent>(second)
        .unwrap()
        .turn
        .is_none());
    stop(&mut app, workspace);
}
#[test]
fn persistent_session_reopens_and_temporary_entry_does_not_replay() {
    let root = tempfile::tempdir().unwrap();
    driver(root.path(), "base", DRIVER, &[]);
    let mut app = app(root.path());
    let workspace = start(&mut app, definition(root.path(), &["base"])).unwrap();
    let target = id("agent:local/agent-0");
    let entity = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1;
    let instance = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .lua
        .instance
        .clone()
        .unwrap();
    command(
        &mut app,
        &target,
        "persist-session",
        "INJECT ? TO session.processed",
        vec![serde_json::json!([{"type":"user", "content":"persistent"}])],
    )
    .result
    .unwrap();
    tick_until(&mut app, "driver waiting before stale injection", |world| {
        let agent = world.get_component::<Agent>(entity)?;
        let wake = agent.lua.wake.lock().unwrap();
        (wake.waiting && wake.sender.is_some()).then_some(())
    });
    command(
        &mut app,
        &target,
        "stale-entry",
        "INJECT ? TO entry.pending",
        vec![serde_json::json!([{"type":"user", "content":"never replay"}])],
    )
    .result
    .unwrap();
    stop(&mut app, workspace);
    let workspace = start(&mut app, definition(root.path(), &["base"])).unwrap();
    let entity = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1;
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert_ne!(agent.lua.instance.as_ref().unwrap(), &instance);
    assert_eq!(
        stored(agent, "session", "processed"),
        StoredBlockInner::Message(vec![margatroid_types::message_types::AgentMessage::User {
            images: Vec::new(),
            content: "persistent".into(),
        }])
    );
    assert!(stored(agent, "entry", "pending").is_empty());
    assert!(agent
        .history
        .list()
        .unwrap()
        .iter()
        .all(|record| record.kind != "message"));
    assert_eq!(
        command(
            &mut app,
            &target,
            "restart-created",
            "CREATE BLOCK session (processed MESSAGE)",
            vec![]
        )
        .result
        .unwrap(),
        MclValue::BlockOpened {
            block_id: "session".into(),
            created: false,
            temporary: false
        }
    );
    stop(&mut app, workspace);
}
#[test]
fn driver_failure_before_ready_removes_agents_and_keeps_running_workspace() {
    let root = tempfile::tempdir().unwrap();
    driver(
        root.path(),
        "broken",
        "error('intentional early failure')",
        &[],
    );
    let mut app = app(root.path());
    let entity = start(&mut app, definition(root.path(), &["broken", "broken"])).unwrap();
    tick_until(&mut app, "startup failure cleanup", |world| {
        world
            .query_with::<Agent>()
            .result()
            .is_empty()
            .then_some(())
    });
    let workspaces = app.world().query_with::<Workspace>().result();
    assert_eq!(workspaces.len(), 1);
    let workspace = app
        .world()
        .get_component::<Workspace>(workspaces[0])
        .unwrap();
    assert!(matches!(
        workspace.state(),
        margatroid_types::workspace_components::WorkspaceState::Running
    ));
    assert_eq!(workspace.member_failures().len(), 2);
    assert!(workspace.member_failures().values().all(|failure| failure
        .error
        .message()
        .contains("intentional early failure")));
    stop(&mut app, entity);
    let restarted = start(&mut app, definition(root.path(), &["broken", "broken"])).unwrap();
    stop(&mut app, restarted);
}
#[test]
fn missing_import_dependency_returns_failure_without_mount_and_can_stop() {
    let root = tempfile::tempdir().unwrap();
    driver(root.path(), "base", DRIVER, &["tool:local/missing:latest"]);
    let mut app = app(root.path());
    let workspace = start(&mut app, definition(root.path(), &["base"])).unwrap();
    let target = id("agent:local/agent-0");
    assert!(command(
        &mut app,
        &target,
        "failed-import",
        "IMPORT TOOL tool:local/missing:latest AS missing",
        vec![]
    )
    .result
    .is_err());
    let entity = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1;
    let set = app.world().get_component::<resource_id_plugin::ResourceSet<margatroid_types::resource_components::ToolResource>>(entity).unwrap();
    assert!(set.is_empty());
    tick_until(
        &mut app,
        "failed import and driver activity drain",
        |world| {
            let agent = world.get_component::<Agent>(entity).unwrap();
            (agent.execution.activity_count() == 0
                && agent.mcl.pending.is_none()
                && agent.mcl.drained.is_drained())
            .then_some(())
        },
    );
    stop(&mut app, workspace);
}

#[test]
fn client_imports_all_resource_types_with_local_aliases_then_stops() {
    let root = tempfile::tempdir().unwrap();
    let dependencies = [
        "tool:local/sample:latest",
        "prompt:local/sample:latest",
        "sandbox:local/sample:latest",
    ];
    driver(root.path(), "base", DRIVER, &dependencies);
    for (directory, files) in [
        (
            "tools",
            vec![
                (
                    "tool.toml",
                    "schema_version = 1\nname = \"sample\"\ndescription = \"tool\"\n",
                ),
                ("input.schema.json", "{\"type\":\"object\"}"),
                ("main.lua", "error('load must not execute Lua')"),
            ],
        ),
        (
            "prompts",
            vec![
                ("prompt.toml", "role = \"system\"\n"),
                ("content.md", "prompt body"),
            ],
        ),
        ("sandboxes", vec![("policy.json", "{\"policy\":\"test\"}")]),
    ] {
        let directory = root.path().join(directory).join("local/sample/latest");
        fs::create_dir_all(&directory).unwrap();
        for (name, content) in files {
            fs::write(directory.join(name), content).unwrap();
        }
    }
    let mut app = app(root.path());
    let workspace = start(&mut app, definition(root.path(), &["base"])).unwrap();
    let target = id("agent:local/agent-0");
    for (index, dependency) in dependencies.iter().enumerate() {
        let result = command(
            &mut app,
            &target,
            &format!("import-{index}"),
            &format!(
                "IMPORT {} {dependency} AS alias-{index}",
                match index {
                    1 => "MESSAGE",
                    2 => "SANDBOX",
                    _ => "TOOL",
                }
            ),
            vec![],
        );
        assert!(
            matches!(result.result, Ok(MclValue::ResourceImport { resource_id, alias, kind }) if resource_id == id(dependency) && alias == format!("alias-{index}") && kind == match index {
                1 => margatroid_types::block_types::InnerType::Message,
                2 => margatroid_types::block_types::InnerType::Sandbox,
                _ => margatroid_types::block_types::InnerType::Tool,
            })
        );
    }
    let entity = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1;
    macro_rules! alias {
        ($ty:ty, $index:expr) => {
            assert_eq!(
                app.world()
                    .get_component::<resource_id_plugin::ResourceSet<$ty>>(entity)
                    .unwrap()
                    .get(&id(dependencies[$index]))
                    .unwrap()
                    .alias,
                format!("alias-{}", $index)
            );
        };
    }
    alias!(margatroid_types::resource_components::ToolResource, 0);
    alias!(margatroid_types::resource_components::PromptResource, 1);
    alias!(margatroid_types::resource_components::SandboxResource, 2);
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
    stop(&mut app, workspace);
}
