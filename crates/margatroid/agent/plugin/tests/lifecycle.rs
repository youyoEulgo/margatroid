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
            world.insert_component(request.agent, test_tasks());
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
    std::fs::write(directory.join("setting.toml"), settings).unwrap();
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
        app.world_mut()
            .insert_resource(tool_resource_core::ToolResourceSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));
        app.world_mut()
            .insert_resource(sandbox_resource_core::SandboxSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));
        app.world_mut()
            .insert_resource(sandbox_resource_core::SandboxSelectionSyncBinding::new(
                |_| client_core::UpdateProbe::new(|_| {}),
            ));
        app.world_mut()
            .insert_resource(inference_core::ModelSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));

        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(AgentPlugin::new(test_agent_context()).with_resource_roots(root.path()))
            .add_system(RuntimePlugin::UPDATE, fake_domains);
        app.world_mut().insert_resource(FakeDomains::default());
        app.world_mut()
            .insert_resource(inference_core::ModelRoutes::new(BTreeMap::from([(
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
            )])));
        let directory = root.path().join("workspaces/demo");
        std::fs::create_dir_all(&directory).unwrap();
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new(
                "Demo".into(),
                directory.join("workspace.yaml"),
                root.path().join("project"),
                WorkspaceState::Running,
                BTreeSet::new(),
                None,
                crate::test_workspace_context(),
            ),
        );
        app.world_mut()
            .insert_component(workspace, spawn_workspace_executor().unwrap());
        let home = directory.join("agents/coder");
        let parameters = AgentStartParameters {
            initial_settings_path: root
                .path()
                .join("agent-drivers/local/first/latest/setting.toml"),
            ui_path: root.path().join("agent-drivers/local/first/latest/ui"),
            project_root: root.path().join("project"),
            home_root: root.path().join("home"),
            database_path: home.join("state.sqlite"),
            settings_path: home.join("setting.toml"),
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
            owner: self.workspace,
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
fn startup_mounts_resources_in_parallel_without_creating_or_configuring_a_vm() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    let source = fixture
        .app
        .world()
        .get_component::<AgentResources>(agent)
        .unwrap();
    assert_eq!(
        source.parameters.settings_path,
        fixture.parameters.settings_path
    );
    assert!(fixture
        .app
        .world()
        .contains_component::<AgentStorage>(agent));
    assert!(fixture
        .app
        .world()
        .contains_component::<AgentSession>(agent));
    assert!(fixture
        .app
        .world()
        .contains_component::<AgentWaitSlot>(agent));
    assert!(!fixture.app.world().contains_component::<DriverLuaVm>(agent));
    assert!(!fixture
        .app
        .world()
        .contains_component::<AgentInferenceConfig>(agent));
    let domains = fixture.app.world().get_resource::<FakeDomains>().unwrap();
    assert_eq!(domains.tool_frames, domains.inference_frames);
    assert!(domains.vm_starts.is_empty());
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<AgentOwner>(agent)
            .unwrap()
            .0,
        fixture.workspace
    );
    let mut stop = fixture.request_stop(agent, "stop");
    assert_eq!(wait(&mut fixture.app, &mut stop).unwrap(), agent);
    assert!(!fixture.app.world().is_alive(agent));
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
fn lost_domain_initialization_reply_retains_unconfirmed_agent_host() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .hold_inference = true;
    let mut reply = fixture.request_start("start");
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_inference
            .is_empty()
    });
    let requests = std::mem::take(
        &mut fixture
            .app
            .world_mut()
            .get_resource_mut::<FakeDomains>()
            .unwrap()
            .held_inference,
    );
    for request in requests {
        request.reply.lock().unwrap().take();
    }
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::ReplyClosed
    );
    let agent = fixture.app.world().query_with::<Agent>().result()[0];
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(agent)
            .unwrap()
            .state(),
        AgentState::Error
    );
}
#[test]
fn stopping_agent_does_not_cancel_or_stop_an_independent_workspace_vm() {
    let mut fixture = Fixture::new();
    let agent = fixture.start();
    let execution = fixture.app.world_mut().spawn();
    let cancel = CancellationToken::new();
    fixture.app.world_mut().insert_component(
        execution,
        DriverLuaVm {
            instance: DriverLuaVmInstance {
                owner: execution,
                start_id: "independent".into(),
            },
            wake: DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(None)),
            ended: None,
            cancel: cancel.clone(),
        },
    );
    let mut reply = fixture.request_stop(agent, "stop");
    wait(&mut fixture.app, &mut reply).unwrap();
    assert!(!cancel.is_cancelled());
    assert!(fixture
        .app
        .world()
        .contains_component::<DriverLuaVm>(execution));
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_stops
        .is_empty());
}
#[test]
fn restart_stops_pending_and_returned_tasks_and_waits_for_real_cleanup_before_start() {
    for returned in [false, true] {
        let mut fixture = Fixture::new();
        let agent = fixture.start();
        let tasks = fixture
            .app
            .world()
            .get_component::<ToolTasks>(agent)
            .unwrap()
            .clone();
        let (task, _input) = new_tool_task(&tasks);
        if returned {
            task.return_result(Ok(tool_core::ToolExecuteResponse {
                call_id: "call".into(),
                alias: "tool".into(),
                content: "registered".into(),
                images: vec![],
                failed: false,
            }))
            .unwrap();
        }
        tasks.register("persistent".into(), task.clone()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (release, waiting) = oneshot::channel();
        let cleanup = tasks.clone();
        let handle = task.clone();
        task.attach_task(runtime.spawn(async move {
            waiting.await.unwrap();
            cleanup.complete_cleanup("persistent", &handle, None);
        }))
        .unwrap();
        let mut response = fixture.request_restart("restart", fixture.parameters.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !task.cancel_token().is_cancelled() {
            fixture.app.tick();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(response.try_recv().is_err());
        assert_eq!(
            *fixture.app.world().agent_state(agent).unwrap(),
            AgentState::Stopping
        );
        assert!(fixture
            .app
            .world()
            .get_component::<AgentSession>(agent)
            .is_some());
        release.send(()).unwrap();
        let new_agent = wait(&mut fixture.app, &mut response).unwrap();
        assert_ne!(agent, new_agent);
        assert!(!fixture.app.world().is_alive(agent));
        assert!(task.is_cleanup_complete());
        let mut stop = fixture.request_stop(new_agent, "stop");
        wait(&mut fixture.app, &mut stop).unwrap();
    }
}
#[test]
fn restart_creates_new_entity_and_preserves_database_settings() {
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
    let (completed, _input) = new_tool_task(
        fixture
            .app
            .world()
            .get_component::<ToolTasks>(agent)
            .unwrap(),
    );
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
        .get_component::<AgentSession>(agent)
        .unwrap()
        .0
        .clone();
    let mut parameters = fixture.parameters.clone();
    parameters.initial_settings_path = fixture
        .root
        .path()
        .join("agent-drivers/local/second/latest/setting.toml");
    parameters.ui_path = fixture
        .root
        .path()
        .join("agent-drivers/local/second/latest/ui");
    let mut reply = fixture.request_restart("restart", parameters);
    let previous = agent;
    let agent = wait(&mut fixture.app, &mut reply).unwrap();
    assert_ne!(agent, previous);
    assert!(!fixture.app.world().is_alive(previous));
    assert_ne!(
        fixture
            .app
            .world()
            .get_component::<AgentSession>(agent)
            .unwrap()
            .0,
        old
    );
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_starts
        .is_empty());
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Starting
    );
    assert!(fixture
        .app
        .world()
        .get_component::<ToolTasks>(agent)
        .unwrap()
        .get("completed")
        .is_err());
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
        turn: Some("turn".into()),
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
    let (tool, _input) = new_tool_task(
        fixture
            .app
            .world()
            .get_component::<ToolTasks>(agent)
            .unwrap(),
    );
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
    let drain = fixture
        .app
        .world()
        .get_component::<MclQueue>(agent)
        .unwrap()
        .drain
        .clone();
    drain.started();
    let mut stop = fixture.request_stop(agent, "stop");
    let mut restart = fixture.request_restart("restart", fixture.parameters.clone());
    fixture.app.tick();
    assert_eq!(
        *fixture.app.world().agent_state(agent).unwrap(),
        AgentState::Stopping
    );
    assert_eq!(
        wait(&mut fixture.app, &mut restart).unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
    let mut duplicate = fixture.request_stop(agent, "duplicate");
    fixture.app.tick();
    assert_eq!(
        duplicate.try_recv().unwrap().unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
    drain.completed(None);
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
    workspace_core::update_workspace(
        fixture.app.world_mut(),
        fixture.workspace,
        workspace_core::WorkspaceChange::State(WorkspaceState::Stopping),
    )
    .unwrap();
    let mut reply = fixture.request_start("stopping");
    assert_eq!(
        wait(&mut fixture.app, &mut reply).unwrap_err().kind,
        AgentErrorKind::StateRejected
    );
}
#[test]
fn missing_initial_config_fails_without_creating_a_vm_and_confirmed_cleanup_removes_host() {
    let mut fixture = Fixture::new();
    let path = fixture
        .root
        .path()
        .join("agent-drivers/local/first/latest/setting.toml");
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
        AgentState::Error
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
        .get_component::<AgentWaitSlot>(agent)
        .unwrap()
        .0
        .lock()
        .unwrap()
        .is_closed());
    assert!(fixture.app.world().is_alive(agent));
    assert!(fixture
        .app
        .world()
        .get_resource::<FakeDomains>()
        .unwrap()
        .vm_stops
        .is_empty());
}

fn new_tool_task(
    tasks: &ToolTasks,
) -> (
    ToolTaskHandle,
    tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
) {
    let (reply, _result) = oneshot::channel();
    ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        reply,
        tasks.update_context(),
        None,
    )
    .unwrap()
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

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}

#[test]
fn stopping_during_starting_waits_for_the_real_initialization_reply() {
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .get_resource_mut::<FakeDomains>()
        .unwrap()
        .hold_inference = true;
    let mut start = fixture.request_start("start");
    advance_until(&mut fixture.app, |world| {
        !world
            .get_resource::<FakeDomains>()
            .unwrap()
            .held_inference
            .is_empty()
    });
    let agent = fixture.app.world().query_with::<Agent>().result()[0];
    let mut stop = fixture.request_stop(agent, "stop-starting");
    for _ in 0..10 {
        fixture.app.tick();
        std::thread::yield_now();
    }
    assert!(stop.try_recv().is_err());
    assert!(fixture.app.world().is_alive(agent));
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(agent)
            .unwrap()
            .state(),
        AgentState::Stopping
    );
    release_inference(&mut fixture.app);
    assert!(wait(&mut fixture.app, &mut start).is_err());
    assert_eq!(wait(&mut fixture.app, &mut stop).unwrap(), agent);
    assert!(!fixture.app.world().is_alive(agent));
}
