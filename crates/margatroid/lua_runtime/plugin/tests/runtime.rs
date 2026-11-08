use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, World};
use lua_runtime_core::*;
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_core::{MclCommand, MclError, MclErrorKind, MclValue, MessageArray};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{WorkspaceExecutionClose, WorkspaceExecutor, spawn_workspace_executor};

#[derive(Clone, Debug)]
struct CommandRecord {
    source: Entity,
    instance: Option<String>,
    target: Entity,
    text: String,
    arguments: Vec<serde_json::Value>,
}

struct Fixture {
    _directory: tempfile::TempDir,
    app: App,
    owner: Entity,
    workspace: Entity,
    close: WorkspaceExecutionClose,
    commands: Arc<Mutex<Vec<CommandRecord>>>,
    exits: Arc<Mutex<Vec<DriverLuaVmExited>>>,
}

impl Fixture {
    fn new(config: LuaRuntimeConfig) -> Self {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(LuaRuntimePlugin::new().with_config(config).unwrap());
        let owner = app.world_mut().spawn();
        app.world_mut().insert_component(
            owner,
            address::Address::new("agent:local/runtime-test:latest").unwrap(),
        );
        let directory = tempfile::tempdir().unwrap();
        let storage = storage_core::open_storage(
            &directory.path().join("agent.sqlite"),
            &directory.path().join("settings.toml"),
            "",
        )
        .unwrap();
        app.world_mut().insert_component(owner, storage);
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace {
                name: "test".into(),
                definition_path: directory.path().join("workspace.yaml"),
                project_root: directory.path().to_owned(),
                agents: Default::default(),
                state: workspace_core::WorkspaceState::Running,
            },
        );
        let executor = spawn_workspace_executor().unwrap();
        let close = executor.close.clone();
        app.world_mut().insert_component(workspace, executor);
        let commands = Arc::new(Mutex::new(Vec::new()));
        let recorded = commands.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            let commands = world
                .event_reader::<MclCommand>()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            for command in commands {
                assert_eq!(command.source_address, "agent:local/runtime-test:latest");
                recorded.lock().unwrap().push(CommandRecord {
                    source: command.source,

                    instance: command.source_instance.clone(),
                    target: command.target,
                    text: command.instruction.text.clone(),
                    arguments: command.instruction.arguments.clone(),
                });
                if command.instruction.text == "HOLD" {
                    continue;
                }
                if command.instruction.text == "LOST_REPLY" {
                    drop(command.reply.lock().unwrap().take());
                    continue;
                }
                if matches!(command.instruction.text.as_str(), "REJECT" | "CANCEL_CALL") {
                    if let Some(sender) = command.reply.lock().unwrap().take() {
                        let kind = if command.instruction.text == "CANCEL_CALL" {
                            MclErrorKind::Cancelled
                        } else {
                            MclErrorKind::EffectFailed
                        };
                        let _ = sender.send(Err(MclError::new(kind, "business failure")));
                    }
                    continue;
                }
                if command.instruction.text == "EFFECT ready" {
                    world
                        .driver_ready(&DriverLuaVmInstance {
                            owner: command.source,
                            start_id: command.source_instance.clone().unwrap(),
                        })
                        .unwrap();
                }
                let value = if command.instruction.text == "ERROR_VALUE" {
                    MclValue::Error(json!({"type": "error", "message": "normalized failure", "request_id": command.id}))
                } else if command.instruction.text == "ECHO" {
                    MclValue::Array(MessageArray::Message(command.instruction.arguments))
                } else {
                    MclValue::Unit
                };
                if let Some(sender) = command.reply.lock().unwrap().take() {
                    let _ = sender.send(Ok(value));
                }
            }
        });
        let exits = Arc::new(Mutex::new(Vec::new()));
        let recorded = exits.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            recorded.lock().unwrap().extend(
                world
                    .event_reader::<DriverLuaVmExited>()
                    .into_iter()
                    .cloned(),
            );
        });
        Self {
            _directory: directory,
            app,
            owner,
            workspace,
            close,
            commands,
            exits,
        }
    }

    fn environment(&self) -> LuaEnvironment {
        LuaEnvironment { globals: BTreeMap::from([("agent_info".into(), LuaBinding::Value(LuaValue::from_json(json!({"entity": {"index": self.owner.index(), "generation": self.owner.generation()}})).unwrap()))]), modules: BTreeMap::new() }
    }

    fn start_request(
        &mut self,
        source: &str,
        environment: LuaEnvironment,
    ) -> oneshot::Receiver<Result<DriverLuaVmInstance, LuaRuntimeError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().send_event(StartDriverLuaVm {
            owner: self.owner,
            workspace: self.workspace,
            program: LuaProgram {
                source: source.into(),
                origin: "test-driver.lua".into(),
                libraries: LuaStandardLibraries::Minimal,
            },
            environment,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }

    async fn start(&mut self, source: &str) -> Result<DriverLuaVmInstance, LuaRuntimeError> {
        let environment = self.environment();
        let mut receiver = self.start_request(source, environment);
        drive(&mut self.app, &mut receiver).await
    }

    fn stop_request(
        &self,
        instance: DriverLuaVmInstance,
    ) -> oneshot::Receiver<Result<(), LuaRuntimeError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().send_event(StopDriverLuaVm {
            instance,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        receiver
    }

    async fn stop(&mut self, instance: DriverLuaVmInstance) -> Result<(), LuaRuntimeError> {
        let mut receiver = self.stop_request(instance);
        drive(&mut self.app, &mut receiver).await
    }

    async fn close(self) {
        tokio::time::timeout(Duration::from_secs(3), self.close.close_and_join())
            .await
            .unwrap()
            .unwrap();
    }
}

async fn drive<T>(app: &mut App, receiver: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.tick();
        match receiver.try_recv() {
            Ok(value) => return value,
            Err(oneshot::error::TryRecvError::Closed) => panic!("reply sender was lost"),
            Err(oneshot::error::TryRecvError::Empty) => (),
        }
        assert!(Instant::now() < deadline, "request did not settle");
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn starts_on_workspace_thread_binds_source_and_stops_without_closing_workspace() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let threads = Arc::new(Mutex::new(Vec::new()));
    let mut environment = fixture.environment();
    environment.globals.insert(
        "record".into(),
        LuaBinding::Function(Arc::new(RecordThread(threads.clone()))),
    );
    let mut receiver = fixture.start_request("assert(io == nil and os == nil and package == nil); assert(load == nil and loadfile == nil and dofile == nil); record(); margatroid.driver:mcl(margatroid.context.agent_info.entity, 'EFFECT ready'); margatroid.driver:wait()", environment);
    let instance = drive(&mut fixture.app, &mut receiver).await.unwrap();
    assert_ne!(threads.lock().unwrap()[0], std::thread::current().id());
    let (sender, receiver) = oneshot::channel();
    fixture
        .app
        .world()
        .get_component::<WorkspaceExecutor>(fixture.workspace)
        .unwrap()
        .entry
        .submit(Box::new(move || -> workspace_core::LocalFuture {
            Box::pin(async move {
                sender.send(std::thread::current().id()).unwrap();
            })
        }))
        .unwrap();
    assert_eq!(receiver.await.unwrap(), threads.lock().unwrap()[0]);
    let commands = fixture.commands.lock().unwrap().clone();
    assert_eq!(commands[0].source, fixture.owner);
    assert_eq!(commands[0].target, fixture.owner);
    assert_eq!(
        commands[0].instance.as_deref(),
        Some(instance.start_id.as_str())
    );
    assert_eq!(commands[0].text, "EFFECT ready");
    fixture.stop(instance).await.unwrap();
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .is_none()
    );
    assert!(
        !fixture
            .app
            .world()
            .get_component::<WorkspaceExecutor>(fixture.workspace)
            .unwrap()
            .entry
            .is_closed()
    );
    fixture.close().await;
}

struct RecordThread(Arc<Mutex<Vec<ThreadId>>>);
impl LuaHostFunction for RecordThread {
    fn call(&self, _: Vec<LuaValue>, _: CancellationToken) -> HostFuture {
        let threads = self.0.clone();
        Box::pin(async move {
            threads.lock().unwrap().push(std::thread::current().id());
            Ok(LuaValue::Nil)
        })
    }
}

#[tokio::test]
async fn startup_timeout_cancels_and_waits_for_vm_cleanup() {
    let mut fixture = Fixture::new(LuaRuntimeConfig {
        startup_timeout: Some(Duration::from_millis(20)),
        ..LuaRuntimeConfig::default()
    });
    let error = fixture.start("margatroid.driver:wait()").await.unwrap_err();
    assert_eq!(error.kind, LuaRuntimeErrorKind::StartupTimeout);
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .is_none()
    );
    fixture.close().await;
}

#[tokio::test]
async fn compile_error_early_exit_and_host_panic_settle_start_after_cleanup() {
    for source in [
        "this is not Lua",
        "error('boot failed')",
        "return",
        "explode()",
    ] {
        let mut fixture = Fixture::new(LuaRuntimeConfig::default());
        let mut environment = fixture.environment();
        environment
            .globals
            .insert("explode".into(), LuaBinding::Function(Arc::new(PanicHost)));
        let mut receiver = fixture.start_request(source, environment);
        let error = drive(&mut fixture.app, &mut receiver).await.unwrap_err();
        assert!(
            matches!(
                error.kind,
                LuaRuntimeErrorKind::CompileFailed | LuaRuntimeErrorKind::ExecutionFailed
            ),
            "{error}"
        );
        assert!(
            fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .is_none()
        );
        let instance = fixture
            .start("margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()")
            .await
            .unwrap();
        fixture.stop(instance).await.unwrap();
        fixture.close().await;
    }
}

struct PanicHost;
impl LuaHostFunction for PanicHost {
    fn call(&self, _: Vec<LuaValue>, _: CancellationToken) -> HostFuture {
        Box::pin(async {
            panic!("host panic");
        })
    }
}

#[tokio::test]
async fn old_instances_cannot_ready_wake_or_stop_a_restarted_vm() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let old = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()")
        .await
        .unwrap();
    fixture.stop(old.clone()).await.unwrap();
    let current = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()")
        .await
        .unwrap();
    assert_ne!(old.start_id, current.start_id);
    assert_eq!(
        fixture.app.world().driver_ready(&old).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceMismatch
    );
    assert_eq!(
        fixture.app.world().driver_wake(&old).unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceMismatch
    );
    assert_eq!(
        fixture.stop(old).await.unwrap_err().kind,
        LuaRuntimeErrorKind::InstanceMismatch
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .unwrap()
            .instance,
        current
    );
    fixture.stop(current).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn duplicate_stop_consumes_original_ended_receiver_only_once() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()")
        .await
        .unwrap();
    let mut first = fixture.stop_request(instance.clone());
    let mut duplicate = fixture.stop_request(instance);
    assert_eq!(
        drive(&mut fixture.app, &mut duplicate)
            .await
            .unwrap_err()
            .kind,
        LuaRuntimeErrorKind::StopAlreadyWaiting
    );
    drive(&mut fixture.app, &mut first).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn lost_ended_notification_keeps_component_and_does_not_fake_exit() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let (sender, ended) = oneshot::channel();
    drop(sender);
    let instance = DriverLuaVmInstance {
        owner: fixture.owner,
        start_id: "lost-ended".into(),
    };
    fixture.app.world_mut().insert_component(
        fixture.owner,
        DriverLuaVm {
            instance: instance.clone(),
            wake: DriverWakeSlot::shared(),
            ready: Arc::new(Mutex::new(None)),
            ended: Some(ended),
            cancel: CancellationToken::new(),
        },
    );
    assert_eq!(
        fixture.stop(instance.clone()).await.unwrap_err().kind,
        LuaRuntimeErrorKind::EndNotificationLost
    );
    let vm = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(fixture.owner)
        .unwrap();
    assert!(vm.cancel.is_cancelled());
    assert!(vm.ended.is_none());
    assert_eq!(
        fixture.stop(instance).await.unwrap_err().kind,
        LuaRuntimeErrorKind::StopAlreadyWaiting
    );
    fixture
        .app
        .world_mut()
        .remove_component::<DriverLuaVm>(fixture.owner);
    fixture.close().await;
}

#[tokio::test]
async fn source_and_environment_validation_leave_no_vm_installed() {
    let mut fixture = Fixture::new(LuaRuntimeConfig {
        max_source_bytes: 4,
        ..LuaRuntimeConfig::default()
    });
    assert_eq!(
        fixture.start("12345").await.unwrap_err().kind,
        LuaRuntimeErrorKind::SourceTooLarge
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .is_none()
    );
    fixture.close().await;
    for name in ["", "mcl", "wait", "require", "margatroid", "bad\nname"] {
        let mut fixture = Fixture::new(LuaRuntimeConfig::default());
        let environment = LuaEnvironment {
            globals: BTreeMap::from([(name.into(), LuaBinding::Value(LuaValue::Nil))]),
            modules: BTreeMap::new(),
        };
        let mut receiver = fixture.start_request("return", environment);
        assert_eq!(
            drive(&mut fixture.app, &mut receiver)
                .await
                .unwrap_err()
                .kind,
            LuaRuntimeErrorKind::EnvironmentFailed
        );
        assert!(
            fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .is_none()
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn closed_workspace_entry_rejects_submission_and_rolls_back_vm() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    fixture
        .app
        .world()
        .get_component::<WorkspaceExecutor>(fixture.workspace)
        .unwrap()
        .entry
        .close_admission();
    assert_eq!(
        fixture.start("return").await.unwrap_err().kind,
        LuaRuntimeErrorKind::Closed
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .is_none()
    );
    fixture.close().await;
}

struct Bridge(Arc<Mutex<Vec<Vec<LuaValue>>>>);
impl LuaHostFunction for Bridge {
    fn call(&self, arguments: Vec<LuaValue>, _: CancellationToken) -> HostFuture {
        let calls = self.0.clone();
        Box::pin(async move {
            if arguments.is_empty() {
                return LuaValue::from_json(
                    json!({"array": [], "object": {}, "nested": [[]], "nulls": [null, {"value": null}]}),
                );
            }
            calls.lock().unwrap().push(arguments);
            Ok(LuaValue::Nil)
        })
    }
}

#[tokio::test]
async fn host_bridge_preserves_empty_arrays_nulls_and_rejects_mixed_sparse_cyclic_tables() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut environment = fixture.environment();
    environment.globals.insert(
        "bridge".into(),
        LuaBinding::Function(Arc::new(Bridge(calls.clone()))),
    );
    let mut receiver = fixture.start_request("local value = bridge(); bridge(value); assert(not pcall(bridge, {[1]=true,x=true})); assert(not pcall(bridge, {[2]=true})); local cycle = {}; cycle.self=cycle; assert(not pcall(bridge, cycle)); margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()", environment);
    let instance = drive(&mut fixture.app, &mut receiver).await.unwrap();
    let values = calls.lock().unwrap().clone();
    assert_eq!(values.len(), 1);
    assert_eq!(
        values[0][0].to_json().unwrap(),
        json!({"array": [], "object": {}, "nested": [[]], "nulls": [null, {"value": null}]})
    );
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn mcl_host_passes_json_parameters_and_rejects_foreign_entity_targets() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture.start("local result = margatroid.driver:mcl(margatroid.context.agent_info.entity, 'ECHO', {{value=42}, {}}); assert(result.Array.Message[1].value == 42); assert(margatroid.driver:mcl({index=999,generation=0}, 'ECHO').Error.type == 'error'); margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()").await.unwrap();
    let records = fixture.commands.lock().unwrap().clone();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].arguments, [json!({"value": 42}), json!({})]);
    assert_eq!(
        records[0].instance.as_deref(),
        Some(instance.start_id.as_str())
    );
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn require_only_resolves_injected_modules() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let mut environment = fixture.environment();
    environment.modules.insert(
        "provided".into(),
        LuaBinding::Value(LuaValue::Object(BTreeMap::from([(
            "value".into(),
            LuaValue::Integer(42),
        )]))),
    );
    let mut receiver = fixture.start_request("assert(require('provided').value == 42); assert(not pcall(require, 'os')); margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()", environment);
    let instance = drive(&mut fixture.app, &mut receiver).await.unwrap();
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn instruction_hook_cancels_running_lua_without_wait_or_host_calls() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); while true do end")
        .await
        .unwrap();
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn natural_exit_is_reported_without_consuming_stop_receiver() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); error('after-ready')")
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.exits.lock().unwrap().is_empty() {
        fixture.app.tick();
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    let exits = fixture.exits.lock().unwrap().clone();
    assert_eq!(exits[0].instance, instance);
    assert_eq!(
        exits[0].result.as_ref().unwrap_err().kind,
        LuaRuntimeErrorKind::ExecutionFailed
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .unwrap()
            .ended
            .is_some()
    );
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn duplicate_start_is_rejected_and_original_instance_remains() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture
        .start("margatroid.driver:mcl('EFFECT ready'); margatroid.driver:wait()")
        .await
        .unwrap();
    assert_eq!(
        fixture.start("return").await.unwrap_err().kind,
        LuaRuntimeErrorKind::InvalidRequest
    );
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .unwrap()
            .instance,
        instance
    );
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn mcl_failures_are_values_and_same_vm_continues_until_explicit_stop() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture.start(r#"
        for _, instruction in ipairs({'REJECT', 'CANCEL_CALL', 'LOST_REPLY', 'ERROR_VALUE'}) do
            local ok, result = pcall(function(...) return margatroid.driver:mcl(...) end, instruction)
            assert(ok and result.Error.type == 'error')
            assert(type(result.Error.request_id) == 'string')
            assert(type(result.Error.message) == 'string')
        end
        for _, arguments in ipairs({{}, {''}, {true}, {'ECHO', {[2]=true}}, {'ECHO', function() end}}) do
            local ok, result = pcall(function(...) return margatroid.driver:mcl(...) end, arguments[1], arguments[2])
            assert(ok and result.Error.type == 'error')
        end
        local cycle = {}; cycle.self = cycle
        assert(margatroid.driver:mcl('ECHO', cycle).Error.type == 'error')
        assert(margatroid.driver:mcl('ECHO', {{after_error=true}}).Array.Message[1].after_error)
        margatroid.driver:mcl('EFFECT ready')
        while true do margatroid.driver:wait() end
    "#).await.unwrap();
    assert!(fixture.exits.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .unwrap()
            .instance,
        instance
    );
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn shared_host_logs_use_current_turn_and_do_not_log_caught_faults_or_cancellation() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    fixture.app.world_mut().insert_component(
        fixture.owner,
        agent_core::Agent {
            name: "trusted-agent".into(),
            workspace: fixture.workspace,
            state: agent_core::AgentState::Working,
            turn: Some("first-turn".into()),
        },
    );
    let instance = fixture
        .start(
            r#"
        assert(mcl == nil and wait == nil and agent_info == nil)
        assert(margatroid.context.kind == 'driver')
        assert(not pcall(function() margatroid.task:finish() end))
        assert(not pcall(function() error('handled') end))
        assert(not pcall(function() margatroid:log('trace', 'bad level') end))
        assert(not pcall(function() margatroid:log('info', 'bad details', {1}) end))
        assert(not pcall(function() margatroid:log('info', 'decoded array', margatroid.data:decode('json', '[]')) end))
        assert(not pcall(function() margatroid:log('info', 'explicit null', margatroid.data.null) end))
        margatroid:log('info', 'first', {answer=42})
        margatroid:log('debug', 'empty details', {})
        margatroid.driver:mcl('EFFECT ready')
        margatroid.driver:wait()
        margatroid:log('warn', 'second')
        margatroid.driver:mcl('SECOND')
        margatroid.driver:wait()
    "#,
        )
        .await
        .unwrap();
    fixture
        .app
        .world_mut()
        .get_component_mut::<agent_core::Agent>(fixture.owner)
        .unwrap()
        .turn = Some("second-turn".into());
    fixture.app.world().driver_wake(&instance).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|record| record.text == "SECOND")
    {
        fixture.app.tick();
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    fixture.stop(instance.clone()).await.unwrap();
    let storage = fixture
        .app
        .world()
        .get_component::<storage_core::AgentStorage>(fixture.owner)
        .unwrap();
    let logs = storage_core::read_agent_logs(storage, None, 100).unwrap();
    assert_eq!(logs.len(), 3);
    assert_eq!(
        logs[0].record.context_json.as_ref().unwrap()["turn"],
        "first-turn"
    );
    assert_eq!(logs[1].record.details_json, Some(json!({})));
    assert_eq!(logs[1].record.level, storage_core::AgentLogLevel::Debug);
    assert_eq!(
        logs[2].record.context_json.as_ref().unwrap()["turn"],
        "second-turn"
    );
    assert_eq!(logs[0].record.details_json, Some(json!({"answer": 42})));
    assert_eq!(logs[2].record.level, storage_core::AgentLogLevel::Warn);
    assert!(logs.iter().all(|log| log.record.raw_error.is_none()));
    assert_eq!(
        logs[0].record.context_json.as_ref().unwrap()["instance"],
        instance.start_id
    );
    fixture.close().await;
}

#[tokio::test]
async fn uncaught_compile_and_runtime_faults_persist_before_start_reply_with_brief_errors() {
    for (source, stage, trace) in [
        ("local =", "compile", false),
        (
            "local function nested() error('specific failure') end; nested()",
            "execute",
            true,
        ),
    ] {
        let mut fixture = Fixture::new(LuaRuntimeConfig::default());
        let error = fixture.start(source).await.unwrap_err();
        let storage = fixture
            .app
            .world()
            .get_component::<storage_core::AgentStorage>(fixture.owner)
            .unwrap();
        let logs = storage_core::read_agent_logs(storage, None, 100).unwrap();
        assert_eq!(logs.len(), 1);
        let record = &logs[0].record;
        assert_eq!(record.stage, stage);
        assert_eq!(record.source, storage_core::AgentLogSource::Driver);
        assert_eq!(record.origin.as_deref(), Some("test-driver.lua"));
        assert!(
            record
                .raw_error
                .as_ref()
                .unwrap()
                .contains("test-driver.lua")
        );
        assert_eq!(record.traceback.is_some(), trace);
        assert!(!error.description.contains("stack traceback:"));
        if trace {
            assert_eq!(error.description, "specific failure");
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn missing_agent_storage_is_a_real_start_error() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    fixture
        .app
        .world_mut()
        .remove_component::<storage_core::AgentStorage>(fixture.owner);
    assert_eq!(
        fixture.start("return").await.unwrap_err().kind,
        LuaRuntimeErrorKind::EnvironmentFailed
    );
    assert!(
        fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(fixture.owner)
            .is_none()
    );
    fixture.close().await;
}

#[tokio::test]
async fn driver_can_use_shared_data_files_processes_and_wait_any() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture.start(r#"
        local value = margatroid.data:decode('json', '{"hello":"world"}')
        assert(value.hello == 'world')
        assert(margatroid.data:decode('base64', margatroid.data:encode('base64', 'payload')) == 'payload')
        margatroid.files:write('driver.txt', 'content')
        assert(margatroid.files:read('driver.txt') == 'content')
        local process = margatroid.process:start('/usr/bin/printf', {'process works'})
        local winner, output = margatroid:wait_any({stdout=function() return process:read() end})
        assert(winner == 'stdout' and output.data == 'process works')
        assert(process:wait().exit_code == 0)
        local running = margatroid.process:start('/bin/cat', {})
        running:write('still running')
        assert(running:read().data == 'still running')
        margatroid.driver:mcl('EFFECT ready')
        margatroid.driver:wait()
    "#).await.unwrap();
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn uncaught_host_callback_keeps_typed_message_and_full_traceback() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let mut environment = fixture.environment();
    environment.globals.insert(
        "bridge".into(),
        LuaBinding::Function(Arc::new(Bridge(Arc::new(Mutex::new(Vec::new()))))),
    );
    let mut receiver = fixture.start_request(
        "local cycle = {}; cycle.self = cycle; bridge(cycle)",
        environment,
    );
    let error = drive(&mut fixture.app, &mut receiver).await.unwrap_err();
    assert_eq!(error.description, "cyclic Lua table cannot be a host value");
    let storage = fixture
        .app
        .world()
        .get_component::<storage_core::AgentStorage>(fixture.owner)
        .unwrap();
    let logs = storage_core::read_agent_logs(storage, None, 100).unwrap();
    assert_eq!(logs.len(), 1);
    assert!(
        logs[0]
            .record
            .raw_error
            .as_ref()
            .unwrap()
            .contains("stack traceback:")
    );
    assert!(
        logs[0]
            .record
            .traceback
            .as_ref()
            .unwrap()
            .contains("test-driver.lua")
    );
    fixture.close().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stop_reaps_driver_process_before_ended_acknowledgement() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut environment = fixture.environment();
    environment.globals.insert(
        "record_process".into(),
        LuaBinding::Function(Arc::new(Bridge(calls.clone()))),
    );
    let mut receiver = fixture.start_request(
        r#"
        local process = margatroid.process:start('/bin/sh', {'-c', 'echo $$; exec cat'})
        record_process(process:read().data)
        margatroid.driver:mcl('EFFECT ready')
        margatroid.driver:wait()
    "#,
        environment,
    );
    let instance = drive(&mut fixture.app, &mut receiver).await.unwrap();
    let pid = match &calls.lock().unwrap()[0][0] {
        LuaValue::String(pid) => pid.trim().parse::<u32>().unwrap(),
        _ => panic!("process did not return its pid"),
    };
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    fixture.stop(instance).await.unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    fixture.close().await;
}

#[tokio::test]
async fn uncaught_invalid_log_details_persist_one_fault_without_recursive_logging() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let error = fixture
        .start("margatroid:log('info', 'bad details', margatroid.data:decode('json', '[]'))")
        .await
        .unwrap_err();
    assert_eq!(error.description, "expected an object");
    let storage = fixture
        .app
        .world()
        .get_component::<storage_core::AgentStorage>(fixture.owner)
        .unwrap();
    let logs = storage_core::read_agent_logs(storage, None, 100).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].record.stage, "execute");
    assert_eq!(logs[0].record.level, storage_core::AgentLogLevel::Error);
    assert_eq!(logs[0].record.message, "expected an object");
    assert!(logs[0].record.details_json.is_none());
    assert!(logs[0].record.raw_error.is_some());
    assert!(logs[0].record.traceback.is_some());
    fixture.close().await;
}

#[tokio::test]
async fn driver_host_denies_tools_and_tasks_and_keeps_context_and_namespaces_readonly() {
    let mut fixture = Fixture::new(LuaRuntimeConfig::default());
    let instance = fixture
        .start(
            r#"
        assert(margatroid.context.kind == 'driver')
        assert(margatroid.version == nil and margatroid.arguments == nil and margatroid.null == nil)
        assert(margatroid.tool == nil)
        for _, name in ipairs({'read_file','write_file','path_info','path_resolve','directory_list',
            'json_encode','json_decode','yaml_encode','yaml_decode','toml_encode','toml_decode',
            'base64_encode','base64_decode','process_open','await_any','task_return','task_finish',
            'task_write','task_replace','task_read','task_send','task_receive','tool_inject'}) do
            assert(margatroid[name] == nil, 'legacy method remains: '..name)
        end
        for _, operation in ipairs({
            function() margatroid.tools:register({}) end,
            function() margatroid.task:return_result('forbidden') end,
            function() margatroid.task:finish('forbidden') end,
            function() margatroid.task:append_output('forbidden') end,
            function() margatroid.task:replace_output('forbidden') end,
            function() margatroid.task:read_output('forbidden') end,
            function() margatroid.task:send('forbidden', {}) end,
            function() margatroid.task:receive() end,
            function() margatroid.context={} end,
            function() margatroid.data.null={} end,
            function() margatroid.files={} end,
            function() margatroid.driver.mcl={} end,
        }) do assert(not pcall(operation)) end
        local context=margatroid.context
        context.kind='tool'
        assert(margatroid.context.kind == 'driver')
        assert(margatroid.data.null ~= nil)
        margatroid.driver:mcl('EFFECT ready')
        margatroid.driver:wait()
    "#,
        )
        .await
        .unwrap();
    fixture.stop(instance).await.unwrap();
    fixture.close().await;
}
