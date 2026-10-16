use crate::*;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::block_storage::{
    AgentBlockStorage, AgentDatabaseHandle, BlockStorageError, BlockStorageRequest,
    StoredBlockResult,
};
use margatroid_types::execution_components::{AgentExecutionScope, ExecutionActivityKind};
use margatroid_types::mcl_events::AgentMclQueue;
use margatroid_types::resource_components::AgentDriverModelConfig;
use margatroid_types::workspace_execution::spawn_workspace_execution;
use resource_id_plugin::ResourceID;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct ArrayRoundTripHost;
impl LuaHostFunction for ArrayRoundTripHost {
    fn call(
        &self,
        arguments: LuaValue,
        _: LuaEnvironmentContext,
        _: CancellationToken,
    ) -> HostFuture {
        Box::pin(async move {
            let LuaValue::Array(arguments) = arguments else {
                panic!("arguments array")
            };
            if arguments.is_empty() {
                return Ok(LuaValue::Object(BTreeMap::from([
                    ("tool_calls".into(), LuaValue::Array(vec![])),
                    ("object".into(), LuaValue::Object(BTreeMap::new())),
                    (
                        "nested".into(),
                        LuaValue::Array(vec![LuaValue::Array(vec![])]),
                    ),
                ])));
            }
            let LuaValue::Object(message) = &arguments[0] else {
                panic!("message object")
            };
            assert_eq!(message["tool_calls"], LuaValue::Array(vec![]));
            assert_eq!(message["object"], LuaValue::Object(BTreeMap::new()));
            assert_eq!(
                message["nested"],
                LuaValue::Array(vec![LuaValue::Array(vec![])])
            );
            assert_eq!(arguments[1], LuaValue::Object(BTreeMap::new()));
            Ok(LuaValue::Nil)
        })
    }
}

#[tokio::test]
async fn host_arrays_round_trip_empty_nested_fields_without_guessing_empty_objects() {
    crate::handler::execute_lua(
        program("local message = bridge(); assert(#message.tool_calls == 0); bridge(message, {})"),
        LuaEnvironment {
            globals: vec![LuaGlobalBinding {
                name: "bridge".into(),
                binding: LuaBindingValue::Function(Arc::new(ArrayRoundTripHost)),
            }],
            modules: vec![],
        },
        context(),
        CancellationToken::new(),
        LuaRuntimeConfig::default(),
    )
    .await
    .unwrap();
}

fn context() -> LuaEnvironmentContext {
    let mut world = World::new();
    LuaEnvironmentContext {
        instance: DriverLuaVmInstance {
            owner: world.spawn(),
            start_request_id: "test".into(),
        },
        values: BTreeMap::new(),
        wake: DriverWakeSlot::shared(),
        execution: AgentExecutionScope::new(),
        mcl: Default::default(),
    }
}
fn program(source: &str) -> LuaProgram {
    LuaProgram {
        source: source.into(),
        origin: Arc::new("test.lua".into()),
        libraries: LuaStandardLibraries::Minimal,
    }
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin::default())
        .add_plugin(LuaRuntimePlugin::default());
    app
}
struct TestHistory;
impl margatroid_types::history_components::AgentHistoryStore for TestHistory {
    fn append_batch(
        &self,
        _: Vec<margatroid_types::history_components::HistoryRecord>,
    ) -> Result<(), margatroid_types::history_components::HistoryError> {
        Ok(())
    }
    fn list(
        &self,
    ) -> Result<
        Vec<margatroid_types::history_components::HistoryRecord>,
        margatroid_types::history_components::HistoryError,
    > {
        Ok(vec![])
    }
}
impl AgentBlockStorage for TestHistory {
    fn execute(&self, _: BlockStorageRequest) -> Result<StoredBlockResult, BlockStorageError> {
        Err(BlockStorageError::InvalidRequest(
            "lifecycle fixture has no block backend".into(),
        ))
    }
}
fn agent(world: &mut World) -> core_plugin::Entity {
    let entity = world.spawn();
    let database = AgentDatabaseHandle::new(TestHistory);
    let agent = Agent {
        resourceid: ResourceID::parse("agent:demo/test:latest").unwrap(),
        info: AgentInfo {
            model: AgentDriverModelConfig {
                model: "test".into(),
                temperature: None,
                max_output_tokens: None,
                top_p: None,
                stop: vec![],
            },
            project_root: ".".into(),
            home_root: ".".into(),
            driver: ResourceID::parse("driver:demo/test:latest").unwrap(),
            driver_dependencies: Arc::from([]),
        },
        workspace: ResourceID::parse("workspace:demo/test:latest").unwrap(),
        sandbox: None,
        lua: AgentLua::default(),
        state: AgentState::Creating {
            request_id: "start".into(),
            vm_request_id: "vm-start".into(),
        },
        history: database.history(),
        database,
        import_bindings: Default::default(),
        mcl: AgentMclQueue::default(),
        execution: AgentExecutionScope::new(),
        turn: None,
    };
    assert!(world.insert_component(entity, agent));
    entity
}

#[tokio::test]
async fn minimal_vm_exposes_base_language_without_host_access() {
    let source = r#"
        assert(type(ipairs) == 'function')
        assert(type(pairs) == 'function')
        assert(type(pcall) == 'function')
        assert(io == nil and os == nil and debug == nil and package == nil)
        assert(dofile == nil and loadfile == nil and load == nil)
        assert(string == nil and table == nil and math == nil)
        local messages = {{type='user'}, {type='assistant'}, {}}
        local kept = 0
        for _, message in ipairs(messages) do
            if type(message.type) == 'string' then kept = kept + 1 end
        end
        assert(kept == 2)
    "#;
    crate::handler::execute_lua(
        program(source),
        LuaEnvironment::default(),
        context(),
        CancellationToken::new(),
        LuaRuntimeConfig::default(),
    )
    .await
    .unwrap();
}

struct RecordingHost {
    calls: Arc<Mutex<Vec<String>>>,
    count: Arc<Mutex<usize>>,
}
impl LuaHostFunction for RecordingHost {
    fn call(
        &self,
        arguments: LuaValue,
        _context: LuaEnvironmentContext,
        cancel: CancellationToken,
    ) -> HostFuture {
        let calls = self.calls.clone();
        let count = self.count.clone();
        Box::pin(async move {
            let LuaValue::Array(arguments) = arguments else {
                panic!("array arguments")
            };
            let LuaValue::String(command) = &arguments[1] else {
                panic!("command")
            };
            calls.lock().unwrap().push(command.clone());
            if command == "GET entry.pending[0]" {
                let mut count = count.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    return Ok(LuaValue::Array(vec![LuaValue::Object(BTreeMap::from([
                        ("type".into(), LuaValue::String("user".into())),
                        ("content".into(), LuaValue::String("test user".into())),
                    ]))]));
                }
                cancel.cancel();
                return Ok(LuaValue::Array(vec![]));
            }
            if command.starts_with("CREATE") {
                return Ok(LuaValue::Object(BTreeMap::from([
                    ("created".into(), LuaValue::Boolean(true)),
                    (
                        "temporary".into(),
                        LuaValue::Boolean(command.starts_with("CREATE TEMP")),
                    ),
                ])));
            }
            if command.starts_with("EMIT EFFECT inference") {
                return Ok(LuaValue::Object(BTreeMap::from([(
                    "message".into(),
                    LuaValue::Object(BTreeMap::from([
                        ("type".into(), LuaValue::String("assistant".into())),
                        ("content".into(), LuaValue::String("test reply".into())),
                    ])),
                )])));
            }
            Ok(LuaValue::Nil)
        })
    }
}
#[tokio::test]
async fn minimal_vm_runs_updated_driver_with_async_host_calls() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let environment = LuaEnvironment {
        globals: vec![
            LuaGlobalBinding {
                name: "agent_info".into(),
                binding: LuaBindingValue::Value(LuaValue::Object(BTreeMap::from([(
                    "id".into(),
                    LuaValue::String("agent:demo/test:latest".into()),
                )]))),
            },
            LuaGlobalBinding {
                name: "mcl".into(),
                binding: LuaBindingValue::Function(Arc::new(RecordingHost {
                    calls: calls.clone(),
                    count: Arc::new(Mutex::new(0)),
                })),
            },
        ],
        modules: vec![],
    };
    let result = crate::handler::execute_lua(
        program(include_str!("../tests/fixtures/base_driver.lua")),
        environment,
        context(),
        CancellationToken::new(),
        LuaRuntimeConfig::default(),
    )
    .await;
    assert_eq!(result.unwrap_err().kind, LuaRuntimeErrorKind::Cancelled);
    let calls = calls.lock().unwrap();
    assert!(calls.iter().any(|call| call == "EMIT EFFECT ready"));
    assert!(calls.iter().any(|call| call
        == "EMIT EFFECT inference (msg.system_prompt, session.conversation) (session.visible)"));
    assert!(calls.iter().any(|call| call == "EMIT EFFECT finish_turn"));
    assert!(calls
        .iter()
        .any(|call| call == "EMIT EFFECT history_append ?"));
    assert!(calls.iter().all(|call| !call.contains(" FROM ")));
    assert!(calls.iter().all(|call| call != "EMIT EFFECT finish"));
}

#[tokio::test]
async fn cancellation_before_execution_never_runs_source() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = crate::handler::execute_lua(
        program("error('must not execute')"),
        LuaEnvironment::default(),
        context(),
        cancel,
        LuaRuntimeConfig::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, LuaRuntimeErrorKind::Cancelled);
}

#[tokio::test]
async fn driver_early_error_waits_for_real_activity_cleanup_before_start_failure() {
    let mut app = app();
    let owner = agent(app.world_mut());
    let activity = app
        .world()
        .get_component::<Agent>(owner)
        .unwrap()
        .execution
        .register(ExecutionActivityKind::Other)
        .unwrap();
    let (entry, close) = spawn_workspace_execution().unwrap();
    crate::handler::handle_start_driver_lua_vm(
        app.world_mut(),
        StartDriverLuaVm {
            id: "vm-start".into(),
            owner,
            program: program("error('boot failure')"),
            environment: LuaEnvironment::default(),
            execution_entry: entry,
        },
    );
    let scope = app
        .world()
        .get_component::<Agent>(owner)
        .unwrap()
        .execution
        .clone();
    while !scope.is_closed() {
        tokio::task::yield_now().await;
    }
    assert!(activity.cancel.is_cancelled());
    assert!(app
        .world()
        .get_component::<Agent>(owner)
        .unwrap()
        .lua
        .ready
        .as_ref()
        .unwrap()
        .sender
        .lock()
        .unwrap()
        .is_some());
    activity.mark_completed().unwrap();
    loop {
        app.tick();
        let outputs = app.world().event_reader::<StartDriverLuaVmResult>();
        if let Some(output) = outputs.into_iter().next() {
            assert_eq!(
                output.result.as_ref().unwrap_err().kind,
                LuaRuntimeErrorKind::ExecutionFailed
            );
            break;
        }
        tokio::task::yield_now().await;
    }
    let agent = app.world_mut().get_component_mut::<Agent>(owner).unwrap();
    assert!(agent.lua.close.is_some());
    agent
        .lua
        .ended
        .take()
        .unwrap()
        .receiver
        .await
        .unwrap()
        .unwrap();
    close.close().await.unwrap();
}

struct PanicHost;
impl LuaHostFunction for PanicHost {
    fn call(&self, _: LuaValue, _: LuaEnvironmentContext, _: CancellationToken) -> HostFuture {
        Box::pin(async { panic!("host panic") })
    }
}
#[tokio::test]
async fn driver_panic_before_ready_still_delivers_cleaned_ended_and_start_error() {
    let mut app = app();
    let owner = agent(app.world_mut());
    let (entry, close) = spawn_workspace_execution().unwrap();
    crate::handler::handle_start_driver_lua_vm(
        app.world_mut(),
        StartDriverLuaVm {
            id: "vm-start".into(),
            owner,
            program: program("explode()"),
            environment: LuaEnvironment {
                globals: vec![LuaGlobalBinding {
                    name: "explode".into(),
                    binding: LuaBindingValue::Function(Arc::new(PanicHost)),
                }],
                modules: vec![],
            },
            execution_entry: entry,
        },
    );
    let ready = app
        .world()
        .get_component::<Agent>(owner)
        .unwrap()
        .lua
        .ready
        .clone()
        .unwrap();
    while ready.sender.lock().unwrap().is_some() {
        tokio::task::yield_now().await;
    }
    let ended = app
        .world_mut()
        .get_component_mut::<Agent>(owner)
        .unwrap()
        .lua
        .ended
        .take()
        .unwrap();
    ended.receiver.await.unwrap().unwrap();
    loop {
        app.tick();
        if let Some(output) = app
            .world()
            .event_reader::<StartDriverLuaVmResult>()
            .into_iter()
            .next()
        {
            assert!(matches!(
                output.result.as_ref().unwrap_err().kind,
                LuaRuntimeErrorKind::TaskFailed | LuaRuntimeErrorKind::ExecutionFailed
            ));
            break;
        }
        tokio::task::yield_now().await;
    }
    close.close().await.unwrap();
}

#[tokio::test]
async fn ready_then_stop_waits_for_activity_and_receipt_draining() {
    let mut app = app();
    let owner = agent(app.world_mut());
    let (entry, close) = spawn_workspace_execution().unwrap();
    crate::handler::handle_start_driver_lua_vm(
        app.world_mut(),
        StartDriverLuaVm {
            id: "vm-start".into(),
            owner,
            program: program("wait()"),
            environment: LuaEnvironment::default(),
            execution_entry: entry,
        },
    );
    let agent = app.world().get_component::<Agent>(owner).unwrap();
    assert!(agent.lua.ready.as_ref().unwrap().complete(Ok(())));
    let activity = agent
        .execution
        .register(ExecutionActivityKind::Inference)
        .unwrap();
    let drain = agent.mcl.drained.clone();
    drain.mark_busy().unwrap();
    let instance = agent.lua.instance.clone().unwrap();
    let close_handle = agent.lua.close.clone().unwrap();
    app.world_mut()
        .get_component_mut::<Agent>(owner)
        .unwrap()
        .state = AgentState::Stopping {
        request_ids: vec!["stop".into()],
        vm_request_id: Some("vm-stop".into()),
    };
    crate::handler::handle_stop_driver_lua_vm(
        app.world_mut(),
        StopDriverLuaVm {
            id: "vm-stop".into(),
            owner,
            instance: instance.clone(),
        },
    );
    assert!(close_handle.is_close_requested());
    assert!(activity.cancel.is_cancelled());
    assert!(app
        .world()
        .get_component::<Agent>(owner)
        .unwrap()
        .lua
        .ended
        .is_none());
    crate::handler::handle_stop_driver_lua_vm(
        app.world_mut(),
        StopDriverLuaVm {
            id: "duplicate-stop".into(),
            owner,
            instance,
        },
    );
    app.tick();
    assert!(app
        .world()
        .event_reader::<StopDriverLuaVmResult>()
        .into_iter()
        .any(|output| output
            .result
            .as_ref()
            .err()
            .is_some_and(|error| error.kind == LuaRuntimeErrorKind::StopAlreadyWaiting)));
    activity.mark_completed().unwrap();
    drain.mark_drained();
    loop {
        app.tick();
        if let Some(output) = app
            .world()
            .event_reader::<StopDriverLuaVmResult>()
            .into_iter()
            .find(|output| output.id == "vm-stop")
        {
            output.result.as_ref().unwrap();
            break;
        }
        tokio::task::yield_now().await;
    }
    close.close().await.unwrap();
}

#[tokio::test]
async fn closed_entry_submission_rolls_back_installed_slots() {
    let mut app = app();
    let owner = agent(app.world_mut());
    let (entry, close) = spawn_workspace_execution().unwrap();
    entry.close_admission();
    crate::handler::handle_start_driver_lua_vm(
        app.world_mut(),
        StartDriverLuaVm {
            id: "vm-start".into(),
            owner,
            program: program("return"),
            environment: LuaEnvironment::default(),
            execution_entry: entry,
        },
    );
    let lua = &app.world().get_component::<Agent>(owner).unwrap().lua;
    assert!(
        lua.instance.is_none() && lua.ready.is_none() && lua.ended.is_none() && lua.close.is_none()
    );
    close.close().await.unwrap();
}
