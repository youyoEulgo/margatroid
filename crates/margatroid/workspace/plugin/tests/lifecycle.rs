use address::Address;
use agent_core::{AgentError, AgentErrorKind, AgentStartParameters, AgentState, StopAgent};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Resource, World};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tool_core::{ToolExecutionLimits, ToolTaskHandle, ToolTasks};
use workspace_core::*;
use workspace_plugin::WorkspacePlugin;

#[test]
fn legacy_agents_map_starts_with_original_state_paths_and_ignores_hidden_backups() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    let source = "name: demo\nproject_root: /projects/demo\nagents:\n  coder:\n    driver: local/coder:latest\n  reviewer:\n    driver: local/reviewer:latest\n";
    std::fs::write(&path, source).unwrap();
    for hidden in [".operations", ".trash", ".backup"] {
        let directory = root.path().join(hidden);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("workspace.yaml"), "broken: [").unwrap();
    }
    let database = path.parent().unwrap().join("agents/coder/state.sqlite");
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    std::fs::write(&database, b"existing state must stay here").unwrap();
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    assert_eq!(app.world().workspace_definitions().unwrap().list().len(), 1);
    let output = start(&mut app, &path, "start-legacy");
    assert_eq!(output.members.len(), 2);
    let starts = &app.world().get_resource::<FakeAgents>().unwrap().starts;
    assert_eq!(starts[0].0, "coder");
    assert!(starts[0]
        .1
        .initial_settings_path
        .ends_with("local/coder/latest/setting.toml"));
    assert_eq!(starts[0].1.database_path, database);
    assert_eq!(
        starts[0].1.settings_path,
        database.parent().unwrap().join("setting.toml")
    );
    assert_eq!(
        std::fs::read(&database).unwrap(),
        b"existing state must stay here"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    app.world().emit_event(StartWorkspace {
        id: "hidden".into(),
        reply: request_reply("hidden"),
        path: root.path().join(".trash/workspace.yaml"),
    });
    assert_eq!(
        wait(&mut app, "hidden").unwrap_err().kind,
        WorkspaceErrorKind::InvalidRequest
    );
    stop(&mut app, output.workspace, "stop-legacy");
    for hidden in [".operations", ".trash", ".backup"] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(hidden).join("workspace.yaml")).unwrap(),
            "broken: ["
        );
    }
}
#[test]
fn legacy_file_is_read_fresh_and_mutually_exclusive_forms_never_start_from_cached_definition() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    std::fs::write(
        &path,
        "name: demo\nproject_root: /project\nagents: {coder: {driver: local/coder:latest}}\n",
    )
    .unwrap();
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let invalid = "name: demo\nproject_root: /project\nagents: {coder: {driver: local/coder:latest}}\nmembers: []\n";
    std::fs::write(&path, invalid).unwrap();
    app.world().emit_event(StartWorkspace {
        id: "changed".into(),
        reply: request_reply("changed"),
        path: path.clone(),
    });
    assert_eq!(
        wait(&mut app, "changed").unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert!(app
        .world()
        .get_resource::<FakeAgents>()
        .unwrap()
        .starts
        .is_empty());
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    assert_eq!(
        app.world()
            .workspace_definitions()
            .unwrap()
            .get(&path)
            .unwrap()
            .members[0]
            .driver,
        "driver:local/coder:latest"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), invalid);
}

#[derive(Default)]
struct FakeAgents {
    starts: Vec<(String, AgentStartParameters)>,
    stops: usize,
    fail_stop: bool,
    close_stop_reply: bool,
    results: BTreeMap<String, Result<WorkspaceOperationOutput, WorkspaceError>>,
}
impl Resource for FakeAgents {}
fn fake_agents(world: &mut World) {
    let starts = world
        .event_reader::<lua_runtime_core::StartDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in starts {
        let Some(reply) = request.reply.lock().unwrap().take() else {
            continue;
        };
        let execution = world
            .get_component::<lua_runtime_core::WorkspaceLuaTask>(request.owner)
            .unwrap();
        let name = execution.name.clone();
        let workspace = execution.workspace;
        let home = world
            .get_component::<Workspace>(workspace)
            .unwrap()
            .definition_path
            .parent()
            .unwrap()
            .join("agents")
            .join(&name);
        let parameters = AgentStartParameters {
            initial_settings_path: request
                .program
                .origin
                .parent()
                .unwrap()
                .join("setting.toml"),
            ui_path: request.program.origin.parent().unwrap().join("ui"),
            project_root: world
                .get_component::<Workspace>(workspace)
                .unwrap()
                .project_root
                .clone(),
            home_root: home.clone(),
            database_path: home.join("state.sqlite"),
            settings_path: home.join("setting.toml"),
        };
        world
            .get_resource_mut::<FakeAgents>()
            .unwrap()
            .starts
            .push((name.clone(), parameters));
        if name == "bad" {
            let _ = reply.send(Err(lua_runtime_core::LuaRuntimeError::new(
                lua_runtime_core::LuaRuntimeErrorKind::ExecutionFailed,
                "fake Lua task failure",
            )));
            continue;
        }
        let entity = world.spawn();
        world.insert_component(
            entity,
            agent_core::Agent::new(
                name,
                workspace,
                AgentState::Running,
                None,
                crate::test_agent_context(),
            ),
        );
        world.insert_component(entity, test_tasks());
        let _ = reply.send(Ok(lua_runtime_core::DriverLuaVmInstance {
            owner: request.owner,
            start_id: "test-task".into(),
        }));
    }
    let stops: Vec<_> = world
        .event_reader::<StopAgent>()
        .into_iter()
        .cloned()
        .collect();
    for request in stops {
        let Some(reply) = request.reply.lock().unwrap().take() else {
            continue;
        };
        let state = world.get_resource_mut::<FakeAgents>().unwrap();
        state.stops += 1;
        if state.close_stop_reply {
            drop(reply);
            continue;
        }
        if state.fail_stop {
            let _ = reply.send(Err(AgentError::new(
                AgentErrorKind::StopFailed,
                "fake unconfirmed cleanup",
            )));
        } else {
            world.despawn(request.agent);
            let _ = reply.send(Ok(request.agent));
        }
    }
}
fn app(plugin: WorkspacePlugin) -> App {
    let mut app = App::new();
    app.world_mut()
        .insert_resource(workspace_core::WorkspaceUpdateContext::new(|_, _| {}));
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(plugin)
        .add_system(RuntimePlugin::UPDATE, fake_agents);
    let definitions = app.world().workspace_definitions().unwrap().clone();
    let source = definitions.source().unwrap();
    let root = if source.is_file() {
        source.parent().unwrap()
    } else {
        source
    };
    for (_, definition) in definitions.list() {
        for member in definition.members {
            let parsed = address::ParsedAddress::parse(&member.driver).unwrap();
            let directory = root
                .join("agent-drivers")
                .join(parsed.scope)
                .join(parsed.name)
                .join(parsed.tag);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("base.lua"),
                "function configure()return {timeout_ms=0}end function execute()end",
            )
            .unwrap();
        }
    }
    app.world_mut().insert_resource(FakeAgents::default());
    app
}
fn wait(app: &mut App, id: &str) -> Result<WorkspaceOperationOutput, WorkspaceError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if let Some(result) = poll_reply(id) {
            return result;
        }
        assert!(Instant::now() < deadline, "workspace result {id} timed out");
        std::thread::yield_now();
    }
}
fn file(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("workspace.yaml")
}
fn write(path: &Path, name: &str, members: &[&str], project: &str) {
    let definition = WorkspaceDefinition {
        name: name.into(),
        project_root: project.into(),
        members: members
            .iter()
            .map(|name| WorkspaceMemberDefinition {
                name: (*name).into(),
                driver: format!("drivers/{name}"),
                home_root: format!("home/{name}").into(),
            })
            .collect(),
    };
    for member in &definition.members {
        let locator = if member.driver.starts_with("driver:") {
            member.driver.clone()
        } else {
            format!("driver:{}", member.driver)
        };
        let parsed = address::ParsedAddress::parse(&locator).unwrap();
        let directory = path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("agent-drivers")
            .join(parsed.scope)
            .join(parsed.name)
            .join(parsed.tag);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("base.lua"),
            "function configure()return {timeout_ms=0}end function execute()end",
        )
        .unwrap();
    }
    std::fs::write(path, serde_yaml::to_string(&definition).unwrap()).unwrap();
}
fn start(app: &mut App, path: &Path, id: &str) -> WorkspaceOperationOutput {
    app.world().emit_event(StartWorkspace {
        id: id.into(),
        reply: request_reply(id),
        path: path.into(),
    });
    wait(app, id).unwrap()
}
fn stop(app: &mut App, workspace: core_plugin::Entity, id: &str) -> WorkspaceOperationOutput {
    app.world().emit_event(StopWorkspace {
        id: id.into(),
        reply: request_reply(id),
        workspace,
    });
    wait(app, id).unwrap()
}
fn manual_join(app: &App, workspace: core_plugin::Entity) {
    let close = app
        .world()
        .get_component::<WorkspaceExecutor>(workspace)
        .unwrap()
        .close
        .clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(close.close_and_join())
        .unwrap();
}
#[test]
fn empty_workspace_creates_address_and_real_executor_then_stops() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "empty");
    write(&path, "Empty display", &[], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    assert_eq!(app.world().entity_count(), 0);
    assert_eq!(app.world().workspace_definitions().unwrap().list().len(), 1);
    let output = start(&mut app, &path, "start");
    assert!(output.members.is_empty());
    assert_eq!(
        *app.world().workspace_state(output.workspace).unwrap(),
        WorkspaceState::Running
    );
    assert_eq!(
        app.world()
            .get_component::<Address>(output.workspace)
            .unwrap()
            .as_str(),
        "workspace:local/empty:latest"
    );
    let close = app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap()
        .close
        .clone();
    stop(&mut app, output.workspace, "stop");
    assert!(close.is_joined());
    assert!(!app.world().is_alive(output.workspace));
}
#[test]
fn start_uses_fresh_file_updates_shared_cache_and_preserves_existing_state_paths() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "old", &[], "old-project");
    let plugin = WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers"));
    let mut app = app(plugin);
    let definitions = app.world().workspace_definitions().unwrap().clone();
    let database = path.parent().unwrap().join("agents/good/state.sqlite");
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    std::fs::write(&database, b"existing SQLite bytes stay untouched").unwrap();
    write(&path, "fresh", &["good", "bad"], "new-project");
    app.world().emit_event(StartWorkspace {
        id: "fresh-start".into(),
        path: path.clone(),
        reply: request_reply("fresh-start"),
    });
    let failure = wait(&mut app, "fresh-start").unwrap_err();
    assert_eq!(failure.kind, WorkspaceErrorKind::MemberFailed);
    assert!(failure.message.contains("bad"));
    let workspace = app.world().query_with::<Workspace>().result()[0];
    assert_eq!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .state,
        WorkspaceState::Error
    );
    assert_eq!(definitions.get(&path).unwrap().name, "fresh");
    let component = app.world().get_component::<Workspace>(workspace).unwrap();
    assert_eq!(component.name, "fresh");
    assert_eq!(component.agents.len(), 1);
    let starts = &app.world().get_resource::<FakeAgents>().unwrap().starts;
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].1.database_path, database);
    assert_eq!(
        starts[0].1.settings_path,
        database.parent().unwrap().join("setting.toml")
    );
    assert_eq!(
        starts[0].1.project_root,
        path.parent().unwrap().join("new-project")
    );
    assert_eq!(
        std::fs::read(database).unwrap(),
        b"existing SQLite bytes stay untouched"
    );
    stop(&mut app, workspace, "stop");
}
#[test]
fn unreadable_fresh_definition_never_starts_from_display_cache() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "cached", &["good"], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    std::fs::write(&path, "broken: [").unwrap();
    app.world().emit_event(StartWorkspace {
        id: "broken".into(),
        reply: request_reply("broken"),
        path: path.clone(),
    });
    assert_eq!(
        wait(&mut app, "broken").unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    assert!(app
        .world()
        .get_resource::<FakeAgents>()
        .unwrap()
        .starts
        .is_empty());
    assert_eq!(
        app.world()
            .workspace_definitions()
            .unwrap()
            .get(&path)
            .unwrap()
            .name,
        "cached"
    );
}
#[test]
fn restart_refuses_unfinished_tasks_on_every_member_without_cancelling_them() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "demo", &["one", "two"], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let output = start(&mut app, &path, "start");
    let (reply, _result) = tokio::sync::oneshot::channel();
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        reply,
        app.world()
            .get_component::<ToolTasks>(output.members[1])
            .unwrap()
            .update_context(),
        None,
    )
    .unwrap();
    let tasks = app
        .world()
        .get_component::<ToolTasks>(output.members[1])
        .unwrap();
    tasks.register("unfinished".into(), task.clone()).unwrap();
    app.world().emit_event(RestartWorkspace {
        id: "busy".into(),
        reply: request_reply("busy"),
        path: path.clone(),
    });
    app.tick();
    assert_eq!(
        *app.world().workspace_state(output.workspace).unwrap(),
        WorkspaceState::Running
    );
    assert!(!app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap()
        .entry
        .is_closed());
    assert_eq!(
        wait(&mut app, "busy").unwrap_err().kind,
        WorkspaceErrorKind::Busy
    );
    assert!(!task.cancel_token().is_cancelled());
    assert_eq!(app.world().get_resource::<FakeAgents>().unwrap().stops, 0);
    app.world()
        .get_component::<ToolTasks>(output.members[1])
        .unwrap()
        .complete_cleanup("unfinished", &task, None);
    write(&path, "new definition", &[], "next-project");
    let old = app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap()
        .close
        .clone();
    app.world().emit_event(RestartWorkspace {
        id: "restart".into(),
        reply: request_reply("restart"),
        path: path.clone(),
    });
    let restarted = wait(&mut app, "restart").unwrap();
    assert_eq!(restarted.workspace, output.workspace);
    assert!(restarted.members.is_empty());
    assert!(old.is_joined());
    assert_eq!(app.world().get_resource::<FakeAgents>().unwrap().stops, 2);
    assert_eq!(
        app.world()
            .get_component::<Workspace>(output.workspace)
            .unwrap()
            .name,
        "new definition"
    );
    stop(&mut app, output.workspace, "stop");
}
#[test]
fn restart_invalid_file_stops_old_runtime_and_reports_new_start_error() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "old", &["good"], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let output = start(&mut app, &path, "start");
    let old = app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap()
        .close
        .clone();
    std::fs::write(&path, "invalid: [").unwrap();
    app.world().emit_event(RestartWorkspace {
        id: "restart".into(),
        reply: request_reply("restart"),
        path: path.clone(),
    });
    assert_eq!(
        wait(&mut app, "restart").unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(
        *app.world().workspace_state(output.workspace).unwrap(),
        WorkspaceState::Error
    );
    assert!(!app.world().is_alive(output.members[0]));
    assert!(old.is_joined());
    assert!(app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap()
        .entry
        .is_closed());
    assert_eq!(
        app.world()
            .workspace_definitions()
            .unwrap()
            .get(&path)
            .unwrap()
            .name,
        "old"
    );
    assert_eq!(app.world().get_resource::<FakeAgents>().unwrap().stops, 1);
    stop(&mut app, output.workspace, "stop");
}
#[test]
fn stop_failure_or_lost_reply_preserves_workspace_and_unjoined_executor() {
    for lost_reply in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = file(root.path(), "demo");
        write(&path, "demo", &["good"], "project");
        let mut app = app(WorkspacePlugin::open(root.path())
            .unwrap()
            .with_program_root(root.path().join("agent-drivers")));
        let output = start(&mut app, &path, "start");
        let state = app.world_mut().get_resource_mut::<FakeAgents>().unwrap();
        state.fail_stop = !lost_reply;
        state.close_stop_reply = lost_reply;
        app.world().emit_event(StopWorkspace {
            id: "stop".into(),
            reply: request_reply("stop"),
            workspace: output.workspace,
        });
        assert_eq!(
            wait(&mut app, "stop").unwrap_err().kind,
            WorkspaceErrorKind::CleanupFailed
        );
        assert!(app.world().is_alive(output.workspace));
        assert!(app.world().is_alive(output.members[0]));
        assert_eq!(
            *app.world().workspace_state(output.workspace).unwrap(),
            WorkspaceState::Error
        );
        assert!(!app
            .world()
            .get_component::<WorkspaceExecutor>(output.workspace)
            .unwrap()
            .close
            .is_joined());
        manual_join(&app, output.workspace);
    }
}
#[test]
fn stop_waits_for_real_workspace_thread_join_before_destroying_host() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "demo", &[], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let output = start(&mut app, &path, "start");
    let executor = app
        .world()
        .get_component::<WorkspaceExecutor>(output.workspace)
        .unwrap();
    let close = executor.close.clone();
    let (release, waiter) = tokio::sync::oneshot::channel();
    executor
        .entry
        .submit(move || {
            Box::pin(async move {
                let local = std::rc::Rc::new(7);
                waiter.await.unwrap();
                assert_eq!(*local, 7);
            })
        })
        .unwrap();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        reply: request_reply("stop"),
        workspace: output.workspace,
    });
    for _ in 0..20 {
        app.tick();
        std::thread::yield_now();
    }
    assert!(app.world().is_alive(output.workspace));
    assert!(!close.is_joined());
    assert!(!app
        .world()
        .get_resource::<FakeAgents>()
        .unwrap()
        .results
        .contains_key("stop"));
    release.send(()).unwrap();
    wait(&mut app, "stop").unwrap();
    assert!(close.is_joined());
    assert!(!app.world().is_alive(output.workspace));
}
#[test]
fn stop_restart_start_order_and_non_running_rejections_are_synchronous() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "demo", &[], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let output = start(&mut app, &path, "start");
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        reply: request_reply("stop"),
        workspace: output.workspace,
    });
    app.world().emit_event(RestartWorkspace {
        id: "restart".into(),
        reply: request_reply("restart"),
        path: path.clone(),
    });
    app.world().emit_event(StartWorkspace {
        id: "duplicate".into(),
        reply: request_reply("duplicate"),
        path: path.clone(),
    });
    app.tick();
    assert_eq!(
        *app.world().workspace_state(output.workspace).unwrap(),
        WorkspaceState::Stopping
    );
    assert_eq!(
        wait(&mut app, "restart").unwrap_err().kind,
        WorkspaceErrorKind::StateRejected
    );
    assert_eq!(
        wait(&mut app, "duplicate").unwrap_err().kind,
        WorkspaceErrorKind::Duplicate
    );
    wait(&mut app, "stop").unwrap();
}
#[test]
fn starting_member_is_stopped_through_actual_agent_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "demo", &["good"], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let output = start(&mut app, &path, "start");
    agent_core::WorldAgentExt::agent_transition(
        app.world_mut(),
        output.members[0],
        AgentState::Starting,
        None,
    )
    .unwrap();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        reply: request_reply("stop"),
        workspace: output.workspace,
    });
    wait(&mut app, "stop").unwrap();
    assert!(!app.world().is_alive(output.workspace));
    assert_eq!(app.world().get_resource::<FakeAgents>().unwrap().stops, 1);
}

#[test]
fn open_validates_all_definitions_before_build_and_start_rejects_external_paths() {
    let root = tempfile::tempdir().unwrap();
    let good = file(root.path(), "good");
    write(&good, "good", &[], "project");
    let bad = file(root.path(), "bad");
    std::fs::write(&bad, "invalid: [").unwrap();
    assert!(WorkspacePlugin::open(root.path()).is_err());
    std::fs::remove_file(&bad).unwrap();
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let external = tempfile::tempdir().unwrap();
    let outside = file(external.path(), "outside");
    write(&outside, "outside", &[], "p");
    app.world().emit_event(StartWorkspace {
        id: "outside".into(),
        reply: request_reply("outside"),
        path: outside,
    });
    assert_eq!(
        wait(&mut app, "outside").unwrap_err().kind,
        WorkspaceErrorKind::InvalidRequest
    );
    assert!(app.world().query_with::<Workspace>().result().is_empty());
}
#[cfg(unix)]
#[test]
fn symlinked_state_path_is_refused_without_replacing_external_data() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "demo");
    write(&path, "demo", &["good"], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let external = root.path().join("external.sqlite");
    std::fs::write(&external, b"do not change").unwrap();
    let state = path.parent().unwrap().join("agents/good/state.sqlite");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&external, &state).unwrap();
    app.world().emit_event(StartWorkspace {
        id: "unsafe".into(),
        reply: request_reply("unsafe"),
        path,
    });
    assert_eq!(
        wait(&mut app, "unsafe").unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(std::fs::read(external).unwrap(), b"do not change");
    assert!(app
        .world()
        .get_resource::<FakeAgents>()
        .unwrap()
        .starts
        .is_empty());
}

#[test]
fn workspace_start_reloads_all_sources_and_rejects_unrelated_invalid_definition() {
    let root = tempfile::tempdir().unwrap();
    let selected = file(root.path(), "selected");
    let other = file(root.path(), "other");
    write(&selected, "old-selected", &[], "project");
    write(&other, "old-other", &[], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    write(&selected, "fresh-selected", &[], "project");
    write(&other, "fresh-other", &[], "project");
    let output = start(&mut app, &selected, "full-reload");
    assert_eq!(
        app.world()
            .workspace_definitions()
            .unwrap()
            .get(&other)
            .unwrap()
            .name,
        "fresh-other"
    );
    stop(&mut app, output.workspace, "stop");
    std::fs::write(&other, "broken: [").unwrap();
    app.world().emit_event(StartWorkspace {
        id: "invalid-other".into(),
        reply: request_reply("invalid-other"),
        path: selected.clone(),
    });
    assert_eq!(
        wait(&mut app, "invalid-other").unwrap_err().kind,
        WorkspaceErrorKind::InvalidDefinition
    );
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    assert_eq!(
        app.world()
            .workspace_definitions()
            .unwrap()
            .get(&other)
            .unwrap()
            .name,
        "fresh-other"
    );
}

type OperationResult = Result<WorkspaceOperationOutput, WorkspaceError>;
thread_local! {static REPLIES:std::cell::RefCell<BTreeMap<String,tokio::sync::oneshot::Receiver<OperationResult>>>=Default::default();}
fn request_reply(id: &str) -> WorkspaceOperationReply {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    REPLIES.with(|rows| rows.borrow_mut().insert(id.to_owned(), receiver));
    std::sync::Arc::new(std::sync::Mutex::new(Some(sender)))
}
fn poll_reply(id: &str) -> Option<OperationResult> {
    REPLIES.with(|rows| {
        rows.borrow_mut()
            .get_mut(id)
            .and_then(|receiver| receiver.try_recv().ok())
    })
}

#[test]
fn removed_definition_remains_on_running_component_and_only_stop_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let path = file(root.path(), "removed");
    write(&path, "retained", &[], "project");
    let mut app = app(WorkspacePlugin::open(root.path())
        .unwrap()
        .with_program_root(root.path().join("agent-drivers")));
    let running = start(&mut app, &path, "start-retained");
    let definitions = app.world().workspace_definitions().unwrap().clone();
    assert_eq!(
        app.world()
            .get_component::<Workspace>(running.workspace)
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .name,
        "retained"
    );
    assert_eq!(path.canonicalize().unwrap(), path);
    std::fs::remove_file(&path).unwrap();
    definitions.reload().unwrap();
    assert!(definitions.get(&path).is_none());
    app.world().emit_event(RestartWorkspace {
        id: "missing-restart".into(),
        path: path.clone(),
        reply: request_reply("missing-restart"),
    });
    assert!(wait(&mut app, "missing-restart").is_err());
    assert_eq!(
        *app.world().workspace_state(running.workspace).unwrap(),
        WorkspaceState::Error
    );
    assert_eq!(
        app.world()
            .get_component::<Workspace>(running.workspace)
            .unwrap()
            .definition
            .as_ref()
            .unwrap()
            .name,
        "retained"
    );
    stop(&mut app, running.workspace, "stop-retained");
    assert!(!app.world().is_alive(running.workspace));
}

fn test_binding() -> tool_core::TaskSyncBinding {
    tool_core::TaskSyncBinding::new(
        client_core::Clients::default(),
        std::sync::Arc::from([]),
        |snapshot| {
            serde_json::to_string(&snapshot)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::EncodeFailed)
        },
    )
}
fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let owner = app.world_mut().spawn();
    tool_core::ToolTasks::new(owner, "agent:test/unit:latest".into(), test_binding())
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}
