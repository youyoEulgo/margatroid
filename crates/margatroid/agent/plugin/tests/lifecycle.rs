use address::Address;
use agent_core::*;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, Resource, World};
use inference_core::*;
use lua_runtime_core::*;
use mcl_core::MclQueue;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use storage_core::{execute_storage, AgentStorage, HistoryRecord, StorageOperation, StorageValue};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_core::*;
use workspace_core::*;

#[derive(Default)]
struct FakeDomains {
    frame: u64,
    tool_frames: Vec<u64>,
    inference_frames: Vec<u64>,
    vm_starts: Vec<StartDriverLuaVm>,
    vm_stops: Vec<DriverLuaVmInstance>,
    fail_tool: bool,
    hold_inference: bool,
    held_inference: Vec<InitInference>,
    hold_ready: bool,
    held_ready: Vec<StartDriverLuaVm>,
    lose_ready: bool,
    fail_stop: bool,
    hold_stop: bool,
    held_stop: Vec<StopDriverLuaVm>,
}
impl Resource for FakeDomains {}
fn fake_domains(world: &mut World) {
    world.get_resource_mut::<FakeDomains>().unwrap().frame += 1;
    let tools: Vec<_> = world
        .event_reader::<InitToolTasks>()
        .into_iter()
        .cloned()
        .collect();
    for request in tools {
        let state = world.get_resource_mut::<FakeDomains>().unwrap();
        let frame = state.frame;
        state.tool_frames.push(frame);
        let fail = state.fail_tool;
        if !world.contains_component::<ToolTasks>(request.agent) {
            world.insert_component(request.agent, ToolTasks::default());
        }
        if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(if fail {
                Err(ToolError::new(
                    ToolErrorKind::SubmitFailed,
                    "tool init failed",
                ))
            } else {
                Ok(())
            });
        }
    }
    let inference: Vec<_> = world
        .event_reader::<InitInference>()
        .into_iter()
        .cloned()
        .collect();
    for request in inference {
        let state = world.get_resource_mut::<FakeDomains>().unwrap();
        let frame = state.frame;
        state.inference_frames.push(frame);
        if state.hold_inference {
            state.held_inference.push(request);
            continue;
        }
        if !world.contains_component::<ActiveInference>(request.agent) {
            world.insert_component(request.agent, ActiveInference::default());
        }
        if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(Ok(()));
        }
    }
    let starts: Vec<_> = world
        .event_reader::<StartDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect();
    for request in starts {
        let state = world.get_resource_mut::<FakeDomains>().unwrap();
        state.vm_starts.push(request.clone());
        let count = state.vm_starts.len();
        let lose = state.lose_ready;
        let hold = state.hold_ready;
        if lose {
            request.reply.lock().unwrap().take();
            continue;
        }
        let instance = DriverLuaVmInstance {
            owner: request.owner,
            start_id: format!("fake-vm-{count}"),
        };
        world.insert_component(
            request.owner,
            DriverLuaVm {
                instance: instance.clone(),
                wake: DriverWakeSlot::shared(),
                ready: Arc::new(Mutex::new(None)),
                ended: None,
                cancel: CancellationToken::new(),
            },
        );
        if hold {
            world
                .get_resource_mut::<FakeDomains>()
                .unwrap()
                .held_ready
                .push(request);
        } else if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(Ok(instance));
        }
    }
    let stops: Vec<_> = world
        .event_reader::<StopDriverLuaVm>()
        .into_iter()
        .cloned()
        .collect();
    for request in stops {
        let state = world.get_resource_mut::<FakeDomains>().unwrap();
        state.vm_stops.push(request.instance.clone());
        let fail = state.fail_stop;
        if state.hold_stop {
            state.held_stop.push(request);
            continue;
        }
        let Some(sender) = request.reply.lock().unwrap().take() else {
            continue;
        };
        if fail {
            let _ = sender.send(Err(LuaRuntimeError::new(
                LuaRuntimeErrorKind::CleanupFailed,
                "VM did not confirm exit",
            )));
        } else {
            world.remove_component::<DriverLuaVm>(request.instance.owner);
            let _ = sender.send(Ok(()));
        }
    }
}
fn release_inference(app: &mut App) {
    let requests = std::mem::take(
        &mut app
            .world_mut()
            .get_resource_mut::<FakeDomains>()
            .unwrap()
            .held_inference,
    );
    for request in requests {
        if !app
            .world()
            .contains_component::<ActiveInference>(request.agent)
        {
            app.world_mut()
                .insert_component(request.agent, ActiveInference::default());
        }
        if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(Ok(()));
        }
    }
}
fn release_ready(app: &mut App) {
    let requests = std::mem::take(
        &mut app
            .world_mut()
            .get_resource_mut::<FakeDomains>()
            .unwrap()
            .held_ready,
    );
    for request in requests {
        let instance = app
            .world()
            .get_component::<DriverLuaVm>(request.owner)
            .unwrap()
            .instance
            .clone();
        if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(Ok(instance));
        }
    }
}
fn release_stop(app: &mut App) {
    let requests = std::mem::take(
        &mut app
            .world_mut()
            .get_resource_mut::<FakeDomains>()
            .unwrap()
            .held_stop,
    );
    for request in requests {
        app.world_mut()
            .remove_component::<DriverLuaVm>(request.instance.owner);
        if let Some(sender) = request.reply.lock().unwrap().take() {
            let _ = sender.send(Ok(()));
        }
    }
}
fn advance_until(app: &mut App, condition: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(app.world()) {
        app.tick();
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::yield_now();
    }
}
fn wait(
    app: &mut App,
    receiver: &mut oneshot::Receiver<Result<Entity, AgentError>>,
) -> Result<Entity, AgentError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(oneshot::error::TryRecvError::Closed) => panic!("Agent reply was lost"),
            Err(_) => {}
        }
        assert!(Instant::now() < deadline, "Agent lifecycle timed out");
        std::thread::yield_now();
    }
}
fn driver(root: &Path, name: &str, program: &str, settings: &str) {
    let directory = root.join("agent-drivers/local").join(name).join("latest");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("driver.toml"),"schema_version=1\ndependencies=['tool:local/bash:latest','prompt:local/system:latest']\n[inference]\nmodel='test-model'\ntemperature=0.5\nmax_output_tokens=1024\n").unwrap();
    std::fs::write(directory.join("base.lua"), program).unwrap();
    std::fs::write(directory.join("settings.toml"), settings).unwrap();
}
struct Fixture {
    app: App,
    root: tempfile::TempDir,
    workspace: Entity,
    parameters: AgentStartParameters,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        driver(root.path(), "first", "return 'first'", "mode='original'\n");
        driver(
            root.path(),
            "second",
            "return 'second'",
            "mode='new default'\n",
        );
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(AgentPlugin::new().with_resource_roots(root.path()))
            .add_system(RuntimePlugin::UPDATE, fake_domains);
        app.world_mut().insert_resource(FakeDomains::default());
        app.world_mut().insert_resource(ModelRoutes {
            entries: BTreeMap::from([(
                "test-model".into(),
                ModelRoute {
                    model: "provider-model".into(),
                    provider: "provider".into(),
                    api_type: "chat".into(),
                    base_url: "https://secret.endpoint.invalid".into(),
                    api_key: "NEVER_EXPOSE_THIS_KEY".into(),
                    context_window_tokens: 8192,
                    thinking: Some("enabled".into()),
                    reasoning_effort: Some("medium".into()),
                },
            )]),
        });
        let directory = root.path().join("workspaces/demo");
        std::fs::create_dir_all(&directory).unwrap();
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                name: "Demo".into(),
                definition_path: directory.join("workspace.yaml"),
                project_root: root.path().join("project"),
                state: WorkspaceState::Running,
                agents: BTreeSet::new(),
            },
        );
        app.world_mut()
            .insert_component(workspace, spawn_workspace_executor().unwrap());
        let home = directory.join("agents/coder");
        let parameters = AgentStartParameters {
            driver: "driver:local/first".into(),
            project_root: root.path().join("project"),
            home_root: root.path().join("home"),
            database_path: home.join("state.sqlite"),
            settings_path: home.join("settings.toml"),
        };
        Self {
            app,
            root,
            workspace,
            parameters,
        }
    }
    fn request_start(&self, id: &str) -> oneshot::Receiver<Result<Entity, AgentError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(StartAgent {
            id: id.into(),
            workspace: self.workspace,
            name: "coder".into(),
            parameters: self.parameters.clone(),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }
    fn start(&mut self) -> Entity {
        let mut receiver = self.request_start("start");
        wait(&mut self.app, &mut receiver).unwrap()
    }
    fn request_stop(
        &self,
        agent: Entity,
        id: &str,
    ) -> oneshot::Receiver<Result<Entity, AgentError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(StopAgent {
            id: id.into(),
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }
    fn request_restart(
        &self,
        id: &str,
        parameters: AgentStartParameters,
    ) -> oneshot::Receiver<Result<Entity, AgentError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(RestartAgent {
            id: id.into(),
            workspace: self.workspace,
            name: "coder".into(),
            parameters,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(executor) = self
            .app
            .world()
            .get_component::<WorkspaceExecutor>(self.workspace)
        {
            let close = executor.close.clone();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(close.close_and_join())
                .unwrap();
        }
    }
}
#[test]
fn startup_parallel_init_same_frame_vm_ready_and_safe_agent_info() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .hold_ready = true;
    let mut reply = fixture.request_start("start");
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_ready
            .is_empty()
    });
    let agent = fixture.app.world().query_with::<Agent>().result()[0];
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Starting
    );
    assert!(reply.try_recv().is_err());
    let state = fixture.app.world().get_resource::<FakeDomains>().unwrap();
    assert_eq!(state.tool_frames, state.inference_frames);
    let request = &state.vm_starts[0];
    let LuaBinding::Value(LuaValue::Object(info)) = &request.environment.globals["agent_info"]
    else {
        panic!()
    };
    assert_eq!(
        info["entity"],
        LuaValue::Object(BTreeMap::from([
            ("index".into(), LuaValue::Integer(agent.index() as i64)),
            (
                "generation".into(),
                LuaValue::Integer(agent.generation() as i64)
            )
        ]))
    );
    let LuaValue::Object(model) = &info["model"] else {
        panic!()
    };
    assert_eq!(model["context_window_tokens"], LuaValue::Integer(8192));
    let encoded = format!("{:?}", request.environment);
    assert!(!encoded.contains("NEVER_EXPOSE"));
    assert!(!encoded.contains("secret.endpoint"));
    assert!(!request.environment.globals.contains_key("mcl"));
    assert!(!request.environment.globals.contains_key("wait"));
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<MclQueue>(agent)
            .unwrap()
            .allowed_imports,
        vec!["tool:local/bash:latest", "prompt:local/system:latest"]
    );
    release_ready(&mut fixture.app);
    assert_eq!(wait(&mut fixture.app, &mut reply).unwrap(), agent);
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Idle
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Address>(agent)
            .unwrap()
            .as_str(),
        "agent:demo/coder:latest"
    );
    let mut stop = fixture.request_stop(agent, "stop");
    assert_eq!(wait(&mut fixture.app, &mut stop).unwrap(), agent);
    assert!(!fixture.app.world().is_alive(agent));
    assert!(fixture
        .app
        .world()
        .workspace_agents(fixture.workspace)
        .unwrap()
        .is_empty());
}
#[test]
fn one_initialization_failure_still_waits_for_other_reply_before_cleanup() {
    let mut fixture = Fixture::new();
    let fake = fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap();
    fake.fail_tool = true;
    fake.hold_inference = true;
    let mut reply = fixture.request_start("start");
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_inference
            .is_empty()
    });
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(fixture.app.world().query_with::<Agent>().result().len(), 1);
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_starts
        .is_empty());
    release_inference(&mut fixture.app);
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::StartFailed
    );
    assert!(fixture
        .app
        .world()
        .query_with::<Agent>()
        .result()
        .is_empty());
}
#[test]
fn lost_vm_start_reply_retains_host_as_cleanup_is_not_proven() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .lose_ready = true;
    let mut reply = fixture.request_start("start");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::ReplyClosed
    );
    let agent = fixture.app.world().query_with::<Agent>().result()[0];
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Starting
    );
    assert!(
        !fixture
            .app
            .world()
            .get_component::<MclQueue>(agent)
            .unwrap()
            .accepting
    );
}
#[test]
fn stop_vm_error_retains_stopping_host_and_runtime_components() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .fail_stop = true;
    let mut reply = fixture.request_stop(agent, "stop");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::CleanupFailed
    );
    assert!(fixture.app.world().is_alive(agent));
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Stopping
    );
    assert!(fixture.app.world().contains_component::<DriverLuaVm>(agent));
    assert!(fixture
        .app
        .world()
        .contains_component::<AgentStorage>(agent));
}
#[test]
fn restart_with_unfinished_tools_rejects_without_cancellation_or_vm_stop() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    let (task, _input) = new_tool_task();
    fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .register("unfinished".into(), task.clone())
        .unwrap();
    let vm_cancel = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .cancel
        .clone();
    let mut reply = fixture.request_restart("restart", fixture.parameters.clone());
    fixture.app.tick();
    assert_eq!(
        reply.try_recv().unwrap().unwrap_err().kind,
        AgentErrorKind::UnfinishedTasks
    );
    assert!(!task.cancel_token().is_cancelled());
    assert!(!vm_cancel.is_cancelled());
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Idle
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<MclQueue>(agent)
            .unwrap()
            .accepting
    );
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_stops
        .is_empty());
    fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .complete_cleanup("unfinished", &task, None);
    let mut stop = fixture.request_stop(agent, "stop");
    wait(&mut fixture.app, &mut stop).unwrap();
}
#[test]
fn restart_uses_new_driver_same_entity_and_preserves_database_settings_without_finished_tasks() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    let storage = fixture
        .app
        .world()
        .get_component::<AgentStorage>(agent)
        .unwrap()
        .clone();
    execute_storage(
        &storage,
        StorageOperation::SetConfig {
            path: "custom.value".into(),
            value: 42.into(),
        },
    )
    .unwrap();
    execute_storage(
        &storage,
        StorageOperation::AppendHistory {
            records: vec![HistoryRecord {
                source: None,
                turn: None,
                kind: "message".into(),
                content: "preserved".into(),
                payload: serde_json::json!({"type":"user","content":"preserved"}),
                created_at_ms: Some(0),
            }],
        },
    )
    .unwrap();
    let (completed, _input) = new_tool_task();
    fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .register("completed".into(), completed.clone())
        .unwrap();
    fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .complete_cleanup("completed", &completed, None);
    let old = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .instance
        .clone();
    let mut parameters = fixture.parameters.clone();
    parameters.driver = "driver:local/second".into();
    let mut reply = fixture.request_restart("restart", parameters);
    assert_eq!(wait(&mut fixture.app, &mut reply).unwrap(), agent);
    assert_ne!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(agent)
            .unwrap()
            .instance,
        old
    );
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Idle
    );
    assert!(fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .get("completed")
        .is_err());
    assert_eq!(
        fixture
            .app
            .world()
            .get_resource::<FakeDomains>()
            .unwrap()
            .vm_starts[1]
            .program
            .source,
        "return 'second'"
    );
    let settings = std::fs::read_to_string(&fixture.parameters.settings_path).unwrap();
    assert!(settings.contains("original"));
    assert!(settings.contains("42"));
    assert!(!settings.contains("new default"));
    let StorageValue::History(history) = execute_storage(
        fixture
            .app
            .world()
            .get_component::<AgentStorage>(agent)
            .unwrap(),
        StorageOperation::ReadHistory,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].created_at_ms, Some(0));
    let mut stop = fixture.request_stop(agent, "stop");
    wait(&mut fixture.app, &mut stop).unwrap();
}
#[test]
fn stop_closes_admission_then_waits_for_vm_inference_tools_and_mcl_drain() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let gate = Arc::new(AtomicUsize::new(0));
    let gate_copy = gate.clone();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let (inference_release, inference_wait) = oneshot::channel();
    let inference_task = runtime.spawn(async move {
        worker_cancel.cancelled().await;
        inference_wait.await.unwrap();
    });
    let inference_handle = InferenceHandle {
        request_id: "inference".into(),
        turn: "turn".into(),
        cancel: cancel.clone(),
        stop_stream: Arc::new(move || {
            gate_copy.fetch_add(1, Ordering::SeqCst);
        }),
        task: Arc::new(inference_core::InferenceTaskState::default()),
    };
    inference_handle.attach_task(inference_task).unwrap();
    fixture
        .app
        .world_mut()
        .get_component_mut::<ActiveInference>(agent)
        .unwrap()
        .current = Some(inference_handle);
    let (tool, _input) = new_tool_task();
    let finished = tool.clone();
    let cleanup_tasks = fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .clone();
    let (tool_release, tool_wait) = oneshot::channel();
    tool.attach_task(runtime.spawn(async move {
        tool_wait.await.unwrap();
        cleanup_tasks.complete_cleanup("running", &finished, None);
    }))
    .unwrap();
    fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .register("running".into(), tool.clone())
        .unwrap();
    let drain = fixture
        .app
        .world()
        .get_component::<MclQueue>(agent)
        .unwrap()
        .drain
        .clone();
    drain.started();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .hold_stop = true;
    let mut reply = fixture.request_stop(agent, "stop");
    fixture.app.tick();
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Stopping
    );
    assert!(
        !fixture
            .app
            .world()
            .get_component::<MclQueue>(agent)
            .unwrap()
            .accepting
    );
    assert!(cancel.is_cancelled());
    assert!(tool.cancel_token().is_cancelled());
    assert!(gate.load(Ordering::SeqCst) > 0);
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_stop
            .is_empty()
    });
    release_stop(&mut fixture.app);
    inference_release.send(()).unwrap();
    tool_release.send(()).unwrap();
    for _ in 0..20 {
        fixture.app.tick();
        std::thread::yield_now();
    }
    assert!(fixture.app.world().is_alive(agent));
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    drain.completed(None);
    assert_eq!(wait(&mut fixture.app, &mut reply).unwrap(), agent);
    assert!(!fixture.app.world().is_alive(agent));
}
#[test]
fn same_frame_stop_precedes_restart_and_duplicate_stop_is_rejected() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .hold_stop = true;
    let mut stop = fixture.request_stop(agent, "stop");
    let mut restart = fixture.request_restart("restart", fixture.parameters.clone());
    fixture.app.tick();
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Stopping
    );
    assert_eq!(
        restart.try_recv().unwrap().unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
    let mut duplicate = fixture.request_stop(agent, "duplicate");
    fixture.app.tick();
    assert_eq!(
        duplicate.try_recv().unwrap().unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_stop
            .is_empty()
    });
    release_stop(&mut fixture.app);
    wait(&mut fixture.app, &mut stop).unwrap();
}
#[test]
fn managed_database_paths_and_workspace_whitelist_are_checked_before_spawning() {
    let mut fixture = Fixture::new();
    fixture.parameters.database_path = fixture.root.path().join("outside.sqlite");
    let mut reply = fixture.request_start("invalid-path");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::InvalidRequest
    );
    assert!(fixture
        .app
        .world()
        .query_with::<Agent>()
        .result()
        .is_empty());
    fixture.parameters.database_path = fixture
        .parameters
        .settings_path
        .parent()
        .unwrap()
        .join("state.sqlite");
    fixture
        .app
        .world_mut()
        .get_component_mut::<Workspace>(fixture.workspace)
        .unwrap()
        .state = WorkspaceState::Stopping;
    let mut reply = fixture.request_start("stopping");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
}
#[test]
fn missing_driver_is_read_fresh_and_confirmed_pre_vm_failure_removes_host() {
    let mut fixture = Fixture::new();
    let path = fixture
        .root
        .path()
        .join("agent-drivers/local/first/latest/base.lua");
    std::fs::remove_file(path).unwrap();
    let mut reply = fixture.request_start("missing");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::StartFailed
    );
    assert!(fixture
        .app
        .world()
        .query_with::<Agent>()
        .result()
        .is_empty());
    assert!(fixture
        .app
        .world()
        .workspace_agents(fixture.workspace)
        .unwrap()
        .is_empty());
}
#[test]
fn exception_closes_admission_and_retains_host_without_faking_cleanup() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    fixture.app.world().emit_event(AgentException {
        id: "exception".into(),
        agent,
        error: AgentError::new(AgentErrorKind::CleanupFailed, "execution failed"),
    });
    fixture.app.tick();
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Stopping
    );
    assert!(
        !fixture
            .app
            .world()
            .get_component::<MclQueue>(agent)
            .unwrap()
            .accepting
    );
    assert!(fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .cancel
        .is_cancelled());
    assert!(fixture.app.world().is_alive(agent));
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_stops
        .is_empty());
}

fn new_tool_task() -> (
    ToolTaskHandle,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    let (reply, _result) = oneshot::channel();
    ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        reply,
    )
    .unwrap()
}
