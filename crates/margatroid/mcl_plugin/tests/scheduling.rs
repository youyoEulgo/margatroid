use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use margatroid_types::lua_runtime_components::{DriverLuaVmInstance, DriverReadySlot};
use margatroid_types::message_types::{AgentMessage, InferenceResponse, StopReason};
use margatroid_types::resource_components::AgentDriverModelConfig;
use mcl_plugin::*;
use resource_id_plugin::{ResourceID, ResourcePlugin, WorldResourceExt};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Store {
    records: Arc<Mutex<Vec<HistoryRecord>>>,
    fail: Arc<Mutex<bool>>,
    fail_messages: Arc<Mutex<bool>>,
    message_batches: Arc<Mutex<usize>>,
}
impl AgentHistoryStore for Store {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        let contains_messages = records.iter().any(|record| record.kind == "message");
        if contains_messages {
            *self.message_batches.lock().unwrap() += 1;
        }
        if *self.fail.lock().unwrap() || (contains_messages && *self.fail_messages.lock().unwrap())
        {
            return Err(HistoryError::WriteFailed("unavailable".into()));
        }
        self.records.lock().unwrap().extend(records);
        Ok(())
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        Ok(self.records.lock().unwrap().clone())
    }
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(MclPlugin::default());
    app.world_mut().register_library::<Agent>("agent").unwrap();
    app
}
fn spawn(app: &mut App, name: &str) -> (Entity, Store) {
    let store = Store::default();
    let id: ResourceID = format!("agent:test/{name}").parse().unwrap();
    let entity = app.world_mut().spawn();
    app.world_mut()
        .insert_library(
            entity,
            Agent {
                resourceid: id,
                info: AgentInfo {
                    model: AgentDriverModelConfig {
                        model: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                        top_p: None,
                        stop: vec![],
                    },
                    project_root: Default::default(),
                    home_root: Default::default(),
                    driver: "driver:test/demo".parse().unwrap(),
                    driver_dependencies: Arc::from([]),
                },
                workspace: "workspace:test/demo".parse().unwrap(),
                sandbox: None,
                lua: Default::default(),
                state: AgentState::Idle,
                history: AgentHistoryHandle::new(store.clone()),
                database: margatroid_types::block_storage::AgentDatabaseHandle::new(
                    memory_plugin::SqliteAgentDatabase::open(":memory:").unwrap(),
                ),
                import_bindings: Default::default(),
                mcl: Default::default(),
                execution: Default::default(),
                turn: None,
            },
        )
        .unwrap();
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .lua
        .instance = Some(DriverLuaVmInstance {
        owner: entity,
        start_request_id: format!("start:{name}"),
    });
    (entity, store)
}
fn pump(app: &mut App, entity: Entity) {
    process_mcl_queue(app.world_mut(), entity);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        app.world_mut().advance_event_frame();
        let outputs = app
            .world()
            .event_reader::<MclAsyncOutput>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        for output in outputs {
            complete_mcl_async(app.world_mut(), output);
        }
        let cancelled = app
            .world()
            .event_reader::<MclCancelledOutput>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        for output in cancelled {
            for result in output.results {
                app.world().send_event(result);
            }
            let agent = app
                .world_mut()
                .get_component_mut::<Agent>(output.entity)
                .unwrap();
            agent.mcl.cancel_audits -= 1;
            if agent.mcl.commands.is_empty() && agent.mcl.pending.is_none() {
                agent.mcl.drained.mark_drained();
            }
        }
        process_mcl_queue(app.world_mut(), entity);
        let agent = app.world().get_component::<Agent>(entity).unwrap();
        if agent.mcl.drained.is_drained() {
            app.world_mut().advance_event_frame();
            break;
        }
        if agent
            .mcl
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.operation_id.starts_with("database:"))
            && agent.mcl.cancel_audits == 0
        {
            app.world_mut().advance_event_frame();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "preparation did not join"
        );
        std::thread::yield_now();
    }
}

fn tick_until_result(app: &mut App, id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        app.tick();
        if app
            .world()
            .event_reader::<MclCommandResult>()
            .into_iter()
            .any(|reply| reply.id == id)
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "missing result {id}");
        std::thread::yield_now();
    }
}

fn read_stored(
    app: &App,
    entity: Entity,
    path: margatroid_types::block_types::BlockPath,
) -> margatroid_types::block_storage::StoredBlockInner {
    use margatroid_types::block_storage::*;
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let storage = agent.database.block_storage();
    let request = BlockStorageRequest {
        context: BlockStorageContext {
            agent: agent.resourceid.clone(),
            instance: agent.lua.instance.clone().unwrap(),
        },
        operation: BlockStorageOperation::Get {
            expressions: vec![BlockExpression::Sources(vec![BlockSource::Selector(
                BlockSelector {
                    path,
                    range: BlockRange::All,
                },
            )])],
        },
        cancel: Default::default(),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let StoredBlockResult::Arrays(mut values) = runtime.block_on(storage.execute(request)).unwrap()
    else {
        panic!()
    };
    values.remove(0)
}

fn request(id: &str, target: &str, text: &str, args: Vec<serde_json::Value>) -> MclCommand {
    MclCommand {
        id: id.into(),
        source: "client:test/frontend".parse().unwrap(),
        target: format!("agent:test/{target}").parse().unwrap(),
        command: MclInstruction {
            text: text.into(),
            arguments: args,
        },
    }
}

fn driver_call(
    app: &App,
    owner: Entity,
    target: &str,
    text: &str,
) -> margatroid_types::lua_environment::HostFuture {
    use margatroid_types::lua_environment::*;
    use margatroid_types::mcl_environment::MclEnvironmentFactory;
    let environment = app
        .world()
        .get_resource::<MclEnvironmentFactory>()
        .unwrap()
        .build();
    let LuaBindingValue::Function(host) = &environment.globals[0].binding else {
        panic!()
    };
    let agent = app.world().get_component::<Agent>(owner).unwrap();
    host.call(
        LuaValue::Array(vec![
            LuaValue::String(format!("agent:test/{target}")),
            LuaValue::String(text.into()),
        ]),
        LuaEnvironmentContext {
            instance: agent.lua.instance.clone().unwrap(),
            values: Default::default(),
            wake: agent.lua.wake.clone(),
            execution: agent.execution.clone(),
            mcl: agent.mcl.drained.clone(),
        },
        Default::default(),
    )
}
fn poll_host(
    future: &mut margatroid_types::lua_environment::HostFuture,
) -> std::task::Poll<
    Result<
        margatroid_types::lua_environment::LuaValue,
        margatroid_types::lua_runtime_components::LuaRuntimeError,
    >,
> {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    future.as_mut().poll(&mut context)
}

#[test]
fn source_cancel_wakes_event_driven_target_and_waits_for_real_reply() {
    use app_runtime_plugin::AppRunExt;
    use async_runtime_plugin::WorldAsyncExt;
    use margatroid_types::inference_events::{
        InferenceError, InferenceErrorKind, InferenceRequest,
    };
    use margatroid_types::lua_environment::*;
    use margatroid_types::mcl_environment::MclEnvironmentFactory;
    let mut app = app();
    let (a, _) = spawn(&mut app, "a");
    let (b, _) = spawn(&mut app, "b");
    let agent = app.world_mut().get_component_mut::<Agent>(b).unwrap();
    agent.new_turn();
    agent.state = AgentState::Working;
    app.world_mut()
        .get_component_mut::<Agent>(a)
        .unwrap()
        .lua
        .instance = Some(DriverLuaVmInstance {
        owner: a,
        start_request_id: "source".into(),
    });
    let source = app.world().get_component::<Agent>(a).unwrap();
    let scope = source.execution.clone();
    let environment = app
        .world()
        .get_resource::<MclEnvironmentFactory>()
        .unwrap()
        .build();
    let LuaBindingValue::Function(host) = &environment.globals[0].binding else {
        panic!()
    };
    let future = host.call(
        LuaValue::Array(vec![
            LuaValue::String("agent:test/b".into()),
            LuaValue::String("EMIT EFFECT inference ? ?".into()),
            LuaValue::Array(vec![LuaValue::Array(vec![]), LuaValue::Array(vec![])]),
        ]),
        LuaEnvironmentContext {
            instance: source.lua.instance.clone().unwrap(),
            values: Default::default(),
            wake: source.lua.wake.clone(),
            execution: scope.clone(),
            mcl: source.mcl.drained.clone(),
        },
        Default::default(),
    );
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let triggered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let events = app.world().event_sender();
    let complete = finished.clone();
    app.world().spawn_async_service(async move {
        let result = future.await;
        assert!(result.is_err());
        complete.store(true, std::sync::atomic::Ordering::SeqCst);
        events.send_event(InternalMclCancellationSignal);
    });
    let check = finished.clone();
    let seen = observed.clone();
    app.add_system(
        RuntimePlugin::POST_UPDATE,
        move |world: &mut core_plugin::World| {
            for event in world.event_reader::<InferenceRequest>() {
                if triggered.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                let request = event.clone();
                let scope = scope.clone();
                let seen = seen.clone();
                world.spawn_async_service(async move {
                    scope.request_close();
                    let cancelled = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        request.cancel.cancelled(),
                    )
                    .await
                    .is_ok();
                    seen.store(cancelled, std::sync::atomic::Ordering::SeqCst);
                    request
                        .reply
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(Err(InferenceError::new(
                            InferenceErrorKind::Cancelled,
                            "actual executor exit",
                        )))
                        .unwrap();
                });
            }
            if check.load(std::sync::atomic::Ordering::SeqCst) {
                panic!("test run completed");
            }
        },
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run()));
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        observed.load(std::sync::atomic::Ordering::SeqCst),
        "source cancel woke sleeping App without target reply"
    );
    assert!(app
        .world()
        .get_component::<Agent>(b)
        .unwrap()
        .mcl
        .drained
        .is_drained());
}

#[test]
fn aborted_effects_return_normal_results_and_finish_waits_for_cleanup() {
    use margatroid_types::block_types::BlockInner;
    use margatroid_types::execution_components::ExecutionActivityKind;
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
    let turn = agent.new_turn();
    agent.state = AgentState::Working;
    let activity = agent
        .execution
        .register_for_turn(ExecutionActivityKind::Inference, Some(turn.clone()))
        .unwrap();
    let instance = agent.lua.instance.clone().unwrap();
    agent.turn = None;
    app.world_mut().insert_component(
        entity,
        margatroid_types::agent_components::AbortedAgentTurn {
            instance,
            turn_id: turn,
        },
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "infer-aborted",
            "a",
            "EMIT EFFECT inference ? ?",
            vec![json!([]), json!([])],
        ),
    );
    pump(&mut app, entity);
    assert!(app
        .world()
        .event_reader::<margatroid_types::inference_events::InferenceRequest>()
        .is_empty());
    let result = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .find(|reply| reply.id == "infer-aborted")
        .unwrap();
    assert_eq!(
        result.result,
        Ok(MclValue::Inference(InferenceResponse::aborted()))
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "tools-aborted",
            "a",
            "EMIT EFFECT tool_call ? ?",
            vec![
                json!([{"type":"assistant","reasoning":null,"content":null,"usage":null,"tool_calls":[
                    {"id":"first","tool_name":"missing","arguments":"{}"},
                    {"id":"second","tool_name":"missing","arguments":"{}"}
                ]}]),
                json!([]),
            ],
        ),
    );
    pump(&mut app, entity);
    assert!(app
        .world()
        .event_reader::<margatroid_types::tool_events::ToolExecuteRequest>()
        .is_empty());
    let result = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .find(|reply| reply.id == "tools-aborted")
        .unwrap();
    let Ok(MclValue::Array(BlockInner::Message(messages))) = &result.result else {
        panic!("{result:?}")
    };
    for (message, id) in messages.iter().zip(["first", "second"]) {
        assert!(
            matches!(message, AgentMessage::ToolCall { tool_call_id, content, failed:true, .. } if tool_call_id == id && content == "Tool call aborted.")
        );
    }
    assert_eq!(messages.len(), 2);
    enqueue_mcl_command(
        app.world_mut(),
        request("blocked-finish", "a", "EMIT EFFECT finish_turn", vec![]),
    );
    pump(&mut app, entity);
    assert!(matches!(
        app.world().get_component::<Agent>(entity).unwrap().state,
        AgentState::Working
    ));
    activity.mark_completed().unwrap();
    enqueue_mcl_command(
        app.world_mut(),
        request("finish", "a", "EMIT EFFECT finish_turn", vec![]),
    );
    pump(&mut app, entity);
    assert!(matches!(
        app.world().get_component::<Agent>(entity).unwrap().state,
        AgentState::Idle
    ));
}

#[test]
fn start_requires_idle_and_client_user_admission_does_not_block_driver_writes() {
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    enqueue_mcl_command(
        app.world_mut(),
        request("start", "a", "EMIT EFFECT start_turn", vec![]),
    );
    pump(&mut app, entity);
    let turn = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .turn
        .clone();
    enqueue_mcl_command(
        app.world_mut(),
        request("start-again", "a", "EMIT EFFECT start_turn", vec![]),
    );
    pump(&mut app, entity);
    assert_eq!(
        app.world().get_component::<Agent>(entity).unwrap().turn,
        turn
    );
    let inject = request(
        "user",
        "a",
        "INJECT ? TO entry.m",
        vec![json!([{"type":"inject","messages":[{"type":"user","content":"busy"}]}])],
    );
    enqueue_mcl_command(app.world_mut(), inject.clone());
    app.world_mut().advance_event_frame();
    assert!(app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .any(|reply| reply.id == "user" && reply.result == Err(MclError::AgentBusy)));
    let mut driver = inject;
    driver.id = "driver".into();
    driver.source = "agent:test/a".parse().unwrap();
    enqueue_mcl_command(app.world_mut(), driver);
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .mcl
            .commands
            .len(),
        1
    );
}

#[test]
fn fifo_sync_and_history_first_failure() {
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    enqueue_mcl_command(
        app.world_mut(),
        request("1", "a", "CREATE BLOCK b (m MESSAGE)", vec![]),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "2",
            "a",
            "INJECT ? TO b.m",
            vec![json!([{"type":"user","content":"hi"}])],
        ),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request("3", "a", "EMIT EFFECT start_turn", vec![]),
    );
    pump(&mut app, entity);
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert!(agent.turn.is_some());
    assert!(agent.mcl.commands.is_empty());
    assert!(agent.mcl.drained.is_drained());
    assert_eq!(store.list().unwrap().len(), 3);
    *store.fail.lock().unwrap() = true;
    enqueue_mcl_command(
        app.world_mut(),
        request("4", "a", "EMIT EFFECT finish_turn", vec![]),
    );
    pump(&mut app, entity);
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .turn
        .is_some());
}

#[test]
fn duplicate_and_bounded_budget_and_independent_agents() {
    let mut app = app();
    let (a, _) = spawn(&mut app, "a");
    let (b, _) = spawn(&mut app, "b");
    for i in 0..65 {
        enqueue_mcl_command(
            app.world_mut(),
            request(&i.to_string(), "a", "EMIT EFFECT finish_turn", vec![]),
        );
    }
    enqueue_mcl_command(
        app.world_mut(),
        request("0", "a", "EMIT EFFECT wake", vec![]),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request("b", "b", "EMIT EFFECT start_turn", vec![]),
    );
    pump(&mut app, a);
    pump(&mut app, b);
    assert_eq!(
        app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .mcl
            .commands
            .len(),
        0
    );
    assert!(app
        .world()
        .get_component::<Agent>(b)
        .unwrap()
        .turn
        .is_some());
    pump(&mut app, a);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
}

#[test]
fn pending_blocks_tail_stop_keeps_head_and_completion_is_correlated() {
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let head = request(
        "head",
        "a",
        "EMIT EFFECT inference ? ?",
        vec![json!([]), json!([])],
    );
    enqueue_mcl_command(app.world_mut(), head.clone());
    enqueue_mcl_command(
        app.world_mut(),
        request("tail", "a", "EMIT EFFECT finish_turn", vec![]),
    );
    let pending = MclPending {
        command_id: "head".into(),
        target_entity: a,
        operation_id: "op".into(),
        cancel: Default::default(),
    };
    {
        let agent = app.world_mut().get_component_mut::<Agent>(a).unwrap();
        agent.mcl.pending = Some(pending.clone());
        agent
            .mcl
            .drained
            .set_pending_cancel(Some(pending.cancel.clone()));
        agent.turn = Some("original".into());
    }
    pump(&mut app, a);
    assert_eq!(store.list().unwrap().len(), 0);
    app.world()
        .get_component::<Agent>(a)
        .unwrap()
        .execution
        .request_close();
    pump(&mut app, a);
    assert!(pending.cancel.is_cancelled());
    assert!(!app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert_eq!(store.list().unwrap().len(), 1);
    let output = MclAsyncOutput {
        request: head,
        pending: pending.clone(),
        turn: Some("original".into()),
        result: Ok(MclAsyncPhase::Completed(MclValue::Inference(
            InferenceResponse {
                message: AgentMessage::Assistant {
                    reasoning: None,
                    content: Some("done".into()),
                    tool_calls: vec![],
                    usage: None,
                    stop_reason: None,
                },
                stop_reason: StopReason::Completed,
            },
        ))),
    };
    let mut stale = output.clone();
    stale.pending.operation_id = "stale".into();
    complete_mcl_async(app.world_mut(), stale);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .pending
        .is_some());
    complete_mcl_async(app.world_mut(), output.clone());
    complete_mcl_async(app.world_mut(), output);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records.iter().all(|record| record.kind == "mcl"));
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
}

#[test]
fn history_append_splits_text_and_preserves_nested_inject_messages() {
    let mut app = app();
    let (agent, store) = spawn(&mut app, "a");
    let messages = json!([
        {"type":"user","content":"hello"},
        {"type":"error","message":"failed"},
        {"type":"inject","messages":[{"type":"inject","messages":[{"type":"user","content":"nested"}]}]}
    ]);
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "split",
            "a",
            "EMIT EFFECT history_append ?",
            vec![messages.clone()],
        ),
    );
    pump(&mut app, agent);
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record.kind == "message"));
    assert_eq!(records[0].content, "hello");
    assert_eq!(records[0].payload, json!({"type":"user"}));
    assert_eq!(records[1].content, "failed");
    assert_eq!(records[1].payload, json!({"type":"error"}));
    assert_eq!(records[2].content, "");
    assert_eq!(records[2].payload, messages[2]);
    assert_eq!(*store.message_batches.lock().unwrap(), 1);
}

#[test]
fn wake_latches_and_ready_delivers_once_and_append_has_no_command_record() {
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let instance = DriverLuaVmInstance {
        owner: a,
        start_request_id: "start".into(),
    };
    let (ready, mut rx) = DriverReadySlot::new(instance.clone());
    {
        let agent = app.world_mut().get_component_mut::<Agent>(a).unwrap();
        agent.lua.instance = Some(instance);
        agent.lua.ready = Some(ready);
    }
    for (id, text, args) in [
        ("wake", "EMIT EFFECT wake", vec![]),
        (
            "append",
            "EMIT EFFECT history_append ?",
            vec![json!([{"type":"user","content":"hi"}])],
        ),
    ] {
        enqueue_mcl_command(app.world_mut(), request(id, "a", text, args));
    }
    pump(&mut app, a);
    let mut first = driver_call(&app, a, "a", "EMIT EFFECT ready");
    let mut second = driver_call(&app, a, "a", "EMIT EFFECT ready");
    assert!(poll_host(&mut first).is_pending());
    assert!(poll_host(&mut second).is_pending());
    for _ in 0..10000 {
        app.tick();
    }
    assert!(poll_host(&mut first).is_ready());
    assert!(poll_host(&mut second).is_ready());
    assert!(rx.try_recv().unwrap().is_ok());
    assert!(
        app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .lua
            .wake
            .lock()
            .unwrap()
            .pending_wake
    );
    let records = store.list().unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(records.iter().filter(|r| r.kind == "mcl").count(), 3);
}

#[test]
fn real_inference_reply_unblocks_fifo_and_keeps_submission_turn() {
    use margatroid_types::inference_events::InferenceRequest;
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let agent = app.world_mut().get_component_mut::<Agent>(a).unwrap();
    agent.turn = Some("submitted".into());
    agent.state = AgentState::Working;
    app.world().send_event(request(
        "infer",
        "a",
        "EMIT EFFECT inference ? ?",
        vec![json!([]), json!([])],
    ));
    app.world()
        .send_event(request("tail", "a", "EMIT EFFECT finish_turn", vec![]));
    let mut inference = None;
    for _ in 0..10000 {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<InferenceRequest>()
            .into_iter()
            .next()
        {
            inference = Some(event.clone());
            break;
        }
    }
    let inference = inference.expect("request sent");
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .pending
        .is_some());
    assert_eq!(store.list().unwrap().len(), 1);
    inference
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(InferenceResponse {
            message: AgentMessage::Assistant {
                reasoning: None,
                content: Some("actual".into()),
                tool_calls: vec![],
                usage: None,
                stop_reason: None,
            },
            stop_reason: StopReason::Completed,
        }))
        .unwrap();
    let mut replies = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained()
        && std::time::Instant::now() < deadline
    {
        app.tick();
        replies.extend(
            app.world()
                .event_reader::<MclCommandResult>()
                .into_iter()
                .cloned(),
        );
        std::thread::yield_now();
    }
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .turn
        .is_none());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.kind == "mcl"));
    assert_eq!(records[0].turn.as_deref(), Some("submitted"));
    let reply = replies.iter().find(|reply| reply.id == "infer").unwrap();
    assert!(reply.history_error.is_none());
    let Ok(MclValue::Inference(response)) = &reply.result else {
        panic!("expected inference response: {reply:?}");
    };
    let message = response.message.clone();
    assert_eq!(serde_json::to_value(&message).unwrap()["content"], "actual");
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "save",
            "a",
            "EMIT EFFECT history_append ?",
            vec![json!([message])],
        ),
    );
    pump(&mut app, a);
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].kind, "message");
    let (content, payload) = margatroid_types::history_components::message_history_parts(&message);
    assert_eq!(records[2].content, content);
    assert_eq!(records[2].payload, payload);
    assert_eq!(records[2].content, "actual");
    assert!(records[2].payload.get("content").is_none());
    assert_eq!(*store.message_batches.lock().unwrap(), 1);
}

#[test]
fn unknown_tools_finish_synchronously_without_pending() {
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "tool",
            "a",
            "EMIT EFFECT tool_call ? ?",
            vec![
                json!([{"type":"assistant","tool_calls":[{"id":"one","tool_name":"missing","arguments":"{}"}]}]),
                json!([]),
            ],
        ),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request("tail", "a", "EMIT EFFECT start_turn", vec![]),
    );
    pump(&mut app, a);
    let agent = app.world().get_component::<Agent>(a).unwrap();
    assert!(agent.mcl.pending.is_none());
    assert!(agent.turn.is_some());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.kind == "mcl"));
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
}

#[test]
fn import_requires_exact_dependency_and_visible_alias_conflicts_prevent_submission() {
    use margatroid_types::resource_components::ToolResource;
    use resource_id_plugin::ResourceSet;
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let mut tools = ResourceSet::new();
    for id in ["tool:test/one", "tool:test/two"] {
        tools.insert(
            id.parse().unwrap(),
            ToolResource {
                alias: "same".into(),
                description: "".into(),
                input_schema: json!({}),
                script_path: Default::default(),
            },
        );
    }
    app.world_mut().insert_component(a, tools);
    bind_tool(&mut app, a, callable("tool:test/one", "same"));
    enqueue_mcl_command(
        app.world_mut(),
        request("import", "a", "IMPORT TOOL tool:test/two AS same", vec![]),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "calls",
            "a",
            "EMIT EFFECT tool_call ? ?",
            vec![
                json!([{"type":"assistant","tool_calls":[{"id":"call","tool_name":"same","arguments":"{}"}]}]),
                json!([
                    callable("tool:test/one", "same"),
                    callable("tool:test/two", "same")
                ]),
            ],
        ),
    );
    pump(&mut app, a);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .pending
        .is_none());
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn import_cancel_keeps_pending_until_real_loader_reply() {
    use margatroid_types::resource_events::{
        LoadToolResource, ResourceLoadError, ResourceLoadErrorKind,
    };
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let id: ResourceID = "tool:test/one".parse().unwrap();
    app.world_mut()
        .get_component_mut::<Agent>(a)
        .unwrap()
        .info
        .driver_dependencies = Arc::from([id]);
    app.world().send_event(request(
        "load",
        "a",
        "IMPORT TOOL tool:test/one AS named",
        vec![],
    ));
    app.world()
        .send_event(request("tail", "a", "EMIT EFFECT wake", vec![]));
    let mut load = None;
    for _ in 0..10000 {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<LoadToolResource>()
            .into_iter()
            .next()
        {
            load = Some(event.clone());
            break;
        }
    }
    let load = load.unwrap();
    assert_eq!(load.alias, "named");
    assert_eq!(load.target, a);
    let activity = load.activity.as_ref().unwrap();
    activity.mark_started().unwrap();
    app.world_mut().get_component_mut::<Agent>(a).unwrap().state = AgentState::Stopping {
        request_ids: vec![],
        vm_request_id: None,
    };
    app.tick();
    assert!(load.cancel.is_cancelled());
    assert!(!app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert_eq!(
        app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .mcl
            .commands
            .len(),
        1
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while store.list().unwrap().len() != 2 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    activity.mark_completed().unwrap();
    load.reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Err(ResourceLoadError::new(
            ResourceLoadErrorKind::Cancelled,
            "cancelled after read",
        )))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained()
        && std::time::Instant::now() < deadline
    {
        app.tick();
        std::thread::yield_now();
    }
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert_eq!(
        app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .execution
            .activity_count(),
        0
    );
}

#[test]
fn driver_ready_is_owner_only_and_receipt_precedes_drained() {
    let mut app = app();
    let (a, _) = spawn(&mut app, "a");
    let (b, _) = spawn(&mut app, "b");
    let ia = DriverLuaVmInstance {
        owner: a,
        start_request_id: "a-start".into(),
    };
    let ib = DriverLuaVmInstance {
        owner: b,
        start_request_id: "b-start".into(),
    };
    let (ready, mut ready_rx) = DriverReadySlot::new(ib.clone());
    app.world_mut()
        .get_component_mut::<Agent>(a)
        .unwrap()
        .lua
        .instance = Some(ia);
    {
        let agent = app.world_mut().get_component_mut::<Agent>(b).unwrap();
        agent.lua.instance = Some(ib);
        agent.lua.ready = Some(ready);
    }
    let mut cross = driver_call(&app, a, "b", "EMIT EFFECT ready");
    assert!(poll_host(&mut cross).is_pending());
    for _ in 0..10000 {
        app.tick();
    }
    assert!(matches!(
        poll_host(&mut cross),
        std::task::Poll::Ready(Err(_))
    ));
    assert!(ready_rx.try_recv().is_err());
    enqueue_mcl_command(
        app.world_mut(),
        request("client-ready", "b", "EMIT EFFECT ready", vec![]),
    );
    pump(&mut app, b);
    assert!(ready_rx.try_recv().is_err());
    let mut own = driver_call(&app, b, "b", "EMIT EFFECT ready");
    assert!(poll_host(&mut own).is_pending());
    let mut observed = false;
    for _ in 0..10000 {
        app.tick();
        if ready_rx.try_recv().is_ok() {
            observed = true;
            break;
        }
    }
    assert!(observed);
    assert!(app
        .world()
        .get_component::<Agent>(b)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert!(
        matches!(poll_host(&mut own), std::task::Poll::Ready(Ok(_))),
        "receipt is already delivered when drained is true"
    );
    app.world_mut()
        .get_component_mut::<Agent>(b)
        .unwrap()
        .lua
        .ready = None;
    let mut repeated = driver_call(&app, b, "b", "EMIT EFFECT ready");
    assert!(poll_host(&mut repeated).is_pending());
    for _ in 0..10000 {
        app.tick();
    }
    assert!(
        matches!(poll_host(&mut repeated), std::task::Poll::Ready(Ok(_))),
        "repeat ready after startup cleared slot is allowed"
    );
    let mut stale = driver_call(&app, b, "b", "EMIT EFFECT ready");
    assert!(poll_host(&mut stale).is_pending());
    app.world_mut()
        .get_component_mut::<Agent>(b)
        .unwrap()
        .lua
        .instance
        .as_mut()
        .unwrap()
        .start_request_id = "new-start".into();
    for _ in 0..10000 {
        app.tick();
    }
    assert!(
        matches!(poll_host(&mut stale), std::task::Poll::Ready(Err(_))),
        "old source instance cannot ready replacement"
    );
}

#[test]
fn message_store_failure_does_not_affect_inference_result_or_trigger_implicit_write() {
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let req = request(
        "head",
        "a",
        "EMIT EFFECT inference ? ?",
        vec![json!([]), json!([])],
    );
    enqueue_mcl_command(app.world_mut(), req.clone());
    let pending = MclPending {
        command_id: "head".into(),
        target_entity: a,
        operation_id: "op".into(),
        cancel: Default::default(),
    };
    app.world_mut()
        .get_component_mut::<Agent>(a)
        .unwrap()
        .mcl
        .pending = Some(pending.clone());
    *store.fail_messages.lock().unwrap() = true;
    complete_mcl_async(
        app.world_mut(),
        MclAsyncOutput {
            request: req.clone(),
            pending,
            turn: None,
            result: Ok(MclAsyncPhase::Completed(MclValue::Inference(
                InferenceResponse {
                    message: AgentMessage::Assistant {
                        reasoning: None,
                        content: Some("real".into()),
                        tool_calls: vec![],
                        usage: None,
                        stop_reason: None,
                    },
                    stop_reason: StopReason::Length,
                },
            ))),
        },
    );
    app.tick();
    let results = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let reply = results.iter().find(|r| r.id == "head").unwrap();
    assert_eq!(reply.source, req.source);
    assert_eq!(reply.target, req.target);
    assert!(reply.history_error.is_none());
    assert!(store.list().unwrap().is_empty());
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
    assert!(
        matches!(&reply.result,Ok(MclValue::Inference(response)) if response.stop_reason==StopReason::Length)
    );
}

#[test]
fn real_tool_batch_returns_ordered_failures_and_only_caller_selected_messages_are_saved() {
    use margatroid_types::resource_components::ToolResource;
    use margatroid_types::tool_events::{
        ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse,
    };
    use resource_id_plugin::ResourceSet;
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    let agent = app.world_mut().get_component_mut::<Agent>(a).unwrap();
    agent.new_turn();
    agent.state = AgentState::Working;
    let mut tools = ResourceSet::new();
    tools.insert(
        "tool:test/one".parse().unwrap(),
        ToolResource {
            alias: "run".into(),
            description: "".into(),
            input_schema: json!({}),
            script_path: Default::default(),
        },
    );
    app.world_mut().insert_component(a, tools);
    bind_tool(&mut app, a, callable("tool:test/one", "run"));
    app.world().send_event(request("batch","a","EMIT EFFECT tool_call ? ?",vec![json!([{"type":"assistant","tool_calls":[{"id":"first","tool_name":"run","arguments":"{}"},{"id":"second","tool_name":"run","arguments":"{}"},{"id":"third","tool_name":"missing","arguments":"{}"}]}]),json!(["run"])]));
    let mut calls = vec![];
    for _ in 0..10000 {
        app.tick();
        calls = app
            .world()
            .event_reader::<ToolExecuteRequest>()
            .into_iter()
            .cloned()
            .collect();
        if !calls.is_empty() {
            break;
        }
    }
    assert_eq!(calls.len(), 2);
    calls[1]
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Err(ToolError::new(
            ToolErrorKind::ExecutionFailed,
            "failure",
        )))
        .unwrap();
    calls[0]
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(ToolExecuteResponse {
            message: AgentMessage::ToolCall {
                resource_id: Some(calls[0].resource.resource_id.clone()),
                tool_call_id: "first".into(),
                tool_name: "run".into(),
                content: "done".into(),
                failed: false,
            },
        }))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained()
        && std::time::Instant::now() < deadline
    {
        app.tick();
        std::thread::yield_now();
    }
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "mcl");
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
    app.tick();
    let replies = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let reply = replies.iter().find(|reply| reply.id == "batch").unwrap();
    assert!(reply.history_error.is_none());
    let Ok(MclValue::Array(margatroid_types::block_types::BlockInner::Message(messages))) =
        &reply.result
    else {
        panic!("expected tool messages: {reply:?}");
    };
    let payloads = messages
        .iter()
        .map(|message| json!(message))
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), 3);
    for (index, id) in ["first", "second", "third"].into_iter().enumerate() {
        assert_eq!(payloads[index]["tool_call_id"], id);
    }
    assert_eq!(payloads[0]["failed"], false);
    assert_eq!(payloads[1]["failed"], true);
    assert_eq!(payloads[2]["failed"], true);
    let selected = vec![messages[0].clone(), messages[2].clone()];
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "save-selected",
            "a",
            "EMIT EFFECT history_append ?",
            vec![json!(selected)],
        ),
    );
    pump(&mut app, a);
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    for (record, message) in records[1..].iter().zip(&selected) {
        let (content, payload) =
            margatroid_types::history_components::message_history_parts(message);
        assert_eq!(record.content, content);
        assert_eq!(record.payload, payload);
        assert!(record.payload.get("content").is_none());
    }
    assert_eq!(*store.message_batches.lock().unwrap(), 1);
}

#[test]
fn partial_tool_batch_abort_preserves_completed_reply_and_finishes_normally() {
    use margatroid_types::agent_components::AbortedAgentTurn;
    use margatroid_types::block_types::BlockInner;
    use margatroid_types::execution_components::ExecutionActivityKind;
    use margatroid_types::inference_events::InferenceRequest;
    use margatroid_types::tool_events::{ToolExecuteRequest, ToolExecuteResponse};
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    bind_tool(&mut app, entity, callable("tool:test/one", "run"));
    let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
    let turn = agent.new_turn();
    agent.state = AgentState::Working;
    app.world().send_event(request(
        "partial",
        "a",
        "EMIT EFFECT tool_call ? ?",
        vec![
            json!([{"type":"assistant","tool_calls":[
                {"id":"first","tool_name":"run","arguments":"{}"},
                {"id":"second","tool_name":"run","arguments":"{}"}
            ]}]),
            json!(["run"]),
        ],
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let calls = loop {
        app.tick();
        let calls = app
            .world()
            .event_reader::<ToolExecuteRequest>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if !calls.is_empty() {
            break calls;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(calls.len(), 2);
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    let activities = calls
        .iter()
        .map(|_| {
            agent
                .execution
                .register_for_turn(ExecutionActivityKind::Sandbox, Some(turn.clone()))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let first = AgentMessage::ToolCall {
        resource_id: Some(calls[0].resource.resource_id.clone()),
        tool_call_id: "first".into(),
        tool_name: "run".into(),
        content: "completed before abort".into(),
        failed: false,
    };
    calls[0]
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(ToolExecuteResponse {
            message: first.clone(),
        }))
        .unwrap();
    activities[0].mark_completed().unwrap();
    app.tick();
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .pending
        .is_some());
    let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
    agent.turn = None;
    agent.execution.cancel_turn(&turn);
    let instance = agent.lua.instance.clone().unwrap();
    app.world_mut().insert_component(
        entity,
        AbortedAgentTurn {
            instance,
            turn_id: turn,
        },
    );
    assert!(activities[1].turn_cancel.is_cancelled());
    assert!(!activities[1].cancel.is_cancelled());
    activities[1].mark_completed().unwrap();
    calls[1]
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(ToolExecuteResponse::aborted(
            Some(calls[1].resource.resource_id.clone()),
            &calls[1].call,
        )))
        .unwrap();
    tick_until_result(&mut app, "partial");
    let reply = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .find(|reply| reply.id == "partial")
        .unwrap()
        .clone();
    let Ok(MclValue::Array(BlockInner::Message(messages))) = &reply.result else {
        panic!("{reply:?}")
    };
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], first);
    assert!(
        matches!(&messages[1], AgentMessage::ToolCall {tool_call_id, content, failed:true, ..} if tool_call_id == "second" && content == "Tool call aborted.")
    );
    assert!(matches!(
        app.world().get_component::<Agent>(entity).unwrap().state,
        AgentState::Working
    ));
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "after-batch",
            "a",
            "EMIT EFFECT inference ? ?",
            vec![json!(messages), json!([])],
        ),
    );
    tick_until_result(&mut app, "after-batch");
    assert!(app.world().event_reader::<InferenceRequest>().is_empty());
    let reply = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .find(|reply| reply.id == "after-batch")
        .unwrap();
    assert_eq!(
        reply.result,
        Ok(MclValue::Inference(InferenceResponse::aborted()))
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "after-cancelled-assistant",
            "a",
            "EMIT EFFECT finish_turn",
            vec![],
        ),
    );
    tick_until_result(&mut app, "after-cancelled-assistant");
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert!(matches!(agent.state, AgentState::Idle));
    assert!(agent.turn.is_none());
    assert!(!agent.execution.is_closed());
}

#[test]
fn sandbox_switch_waits_for_activity_and_cardinality_error_is_non_mutating() {
    use margatroid_types::agent_components::{ImportedValue, WorldAgentExt};
    use margatroid_types::execution_components::ExecutionActivityKind;
    use margatroid_types::resource_components::{SandboxResource, SandboxValue};
    use resource_id_plugin::ResourceSet;
    let mut app = app();
    let (a, _) = spawn(&mut app, "a");
    let mut sandboxes = ResourceSet::new();
    let id: ResourceID = "sandbox:test/one".parse().unwrap();
    sandboxes.insert(
        id.clone(),
        SandboxResource {
            alias: "one".into(),
            policy: Arc::from("policy"),
        },
    );
    app.world_mut().insert_component(a, sandboxes);
    let value = SandboxValue {
        resource_id: id.clone(),
        policy: Arc::from("policy"),
    };
    let agent_id = {
        let agent = app.world_mut().get_component_mut::<Agent>(a).unwrap();
        agent.info.driver_dependencies = Arc::from([id.clone()]);
        agent.resourceid.clone()
    };
    app.world_mut()
        .agent_bind_import(
            &agent_id,
            "one".into(),
            ImportedValue::Sandbox(value.clone()),
        )
        .unwrap();
    let scope = app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .execution
        .clone();
    let activity = scope.register(ExecutionActivityKind::Sandbox).unwrap();
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "busy",
            "a",
            "EMIT EFFECT sandbox_use ?",
            vec![json!(["one"])],
        ),
    );
    pump(&mut app, a);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .sandbox
        .is_none());
    activity.rollback_submission().unwrap();
    enqueue_mcl_command(
        app.world_mut(),
        request("ok", "a", "EMIT EFFECT sandbox_use ?", vec![json!(["one"])]),
    );
    pump(&mut app, a);
    assert_eq!(
        app.world().get_component::<Agent>(a).unwrap().sandbox,
        Some(value.clone())
    );
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "bad",
            "a",
            "EMIT EFFECT sandbox_use ?",
            vec![json!(["one", "one"])],
        ),
    );
    pump(&mut app, a);
    assert_eq!(
        app.world().get_component::<Agent>(a).unwrap().sandbox,
        Some(value)
    );
}

#[test]
fn closed_queue_cancels_even_when_history_fails() {
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
    enqueue_mcl_command(
        app.world_mut(),
        request("cancel", "a", "EMIT EFFECT wake", vec![]),
    );
    *store.fail.lock().unwrap() = true;
    app.world()
        .get_component::<Agent>(a)
        .unwrap()
        .execution
        .request_close();
    pump(&mut app, a);
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert!(
        !app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .lua
            .wake
            .lock()
            .unwrap()
            .pending_wake
    );
}

fn callable(id: &str, alias: &str) -> margatroid_types::resource_components::CallableResource {
    use margatroid_types::resource_components::*;
    CallableResource {
        resource_id: id.parse().unwrap(),
        alias: alias.into(),
        definition: Arc::new(CallableDefinitionSnapshot::Tool(ToolResource {
            alias: alias.into(),
            description: String::new(),
            input_schema: json!({}),
            script_path: Default::default(),
        })),
    }
}

#[test]
fn world_alias_get_inject_reload_preserves_old_block_snapshot() {
    use margatroid_types::agent_components::{ImportedValue, WorldAgentExt};
    use margatroid_types::block_types::{BlockInner, BlockPath};
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    let prompt: ResourceID = "prompt:test/soul".parse().unwrap();
    let agent_id = {
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        agent.info.driver_dependencies = Arc::from([prompt.clone()]);
        agent.resourceid.clone()
    };
    app.world_mut()
        .agent_bind_import(
            &agent_id,
            "soul".into(),
            ImportedValue::Message {
                resource_id: prompt.clone(),
                messages: Arc::from([AgentMessage::System {
                    content: "old".into(),
                }]),
            },
        )
        .unwrap();
    for (id, text) in [
        ("create", "CREATE BLOCK msg (ctx MESSAGE)"),
        ("inject", "INJECT soul, msg.ctx[] TO msg.ctx"),
        ("get", "GET soul, msg.ctx[0]"),
    ] {
        enqueue_mcl_command(app.world_mut(), request(id, "a", text, vec![]));
    }
    pump(&mut app, entity);
    let path = BlockPath {
        block_id: "msg".into(),
        inner_id: "ctx".into(),
    };
    let old = BlockInner::Message(vec![AgentMessage::System {
        content: "old".into(),
    }]);
    assert_eq!(
        read_stored(&app, entity, path.clone()),
        margatroid_types::block_storage::StoredBlockInner::from(old.clone())
    );
    app.world_mut()
        .agent_bind_import(
            &agent_id,
            "new-soul".into(),
            ImportedValue::Message {
                resource_id: prompt,
                messages: Arc::from([AgentMessage::System {
                    content: "new".into(),
                }]),
            },
        )
        .unwrap();
    assert!(app.world().agent_get_import(&agent_id, "soul").is_err());
    assert_eq!(
        app.world().agent_get_import(&agent_id, "new-soul").unwrap(),
        BlockInner::Message(vec![AgentMessage::System {
            content: "new".into()
        }])
    );
    assert_eq!(
        read_stored(&app, entity, path.clone()),
        margatroid_types::block_storage::StoredBlockInner::from(old.clone())
    );
}

#[test]
fn empty_assistant_calls_return_typed_message_and_submit_nothing() {
    use margatroid_types::tool_events::ToolExecuteRequest;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    app.world().send_event(request(
        "empty",
        "a",
        "EMIT EFFECT tool_call ? ?",
        vec![json!([{"type":"assistant","tool_calls":[]}]), json!([])],
    ));
    tick_until_result(&mut app, "empty");
    assert!(app
        .world()
        .event_reader::<ToolExecuteRequest>()
        .into_iter()
        .next()
        .is_none());
    let replies = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .collect::<Vec<_>>();
    assert!(replies.iter().any(|reply| reply.id == "empty"
        && reply.result
            == Ok(MclValue::Array(
                margatroid_types::block_types::BlockInner::Message(vec![])
            ))));
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .pending
        .is_none());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "mcl");
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
}

#[test]
fn all_rejected_tools_return_failures_without_implicit_messages_and_append_once() {
    use margatroid_types::block_types::BlockInner;
    use margatroid_types::tool_events::ToolExecuteRequest;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    let assistant = json!({"type":"assistant","content":"caller decides","tool_calls":[
        {"id":"one","tool_name":"missing","arguments":"{}"},
        {"id":"two","tool_name":"also_missing","arguments":"{}"}
    ]});
    app.world().send_event(request(
        "rejected",
        "a",
        "EMIT EFFECT tool_call ? ?",
        vec![json!([assistant.clone()]), json!([])],
    ));
    tick_until_result(&mut app, "rejected");
    assert!(app.world().event_reader::<ToolExecuteRequest>().is_empty());
    let replies = app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let reply = replies.iter().find(|reply| reply.id == "rejected").unwrap();
    assert!(reply.history_error.is_none());
    let Ok(MclValue::Array(BlockInner::Message(messages))) = &reply.result else {
        panic!("expected rejected tool results: {reply:?}");
    };
    assert_eq!(messages.len(), 2);
    assert_eq!(json!(messages[0])["tool_call_id"], "one");
    assert_eq!(json!(messages[1])["tool_call_id"], "two");
    assert!(messages
        .iter()
        .all(|message| json!(message)["failed"] == true));
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
    assert!(store
        .list()
        .unwrap()
        .iter()
        .all(|record| record.kind == "mcl"));
    let selected = vec![assistant, json!(messages[1])];
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "save",
            "a",
            "EMIT EFFECT history_append ?",
            vec![json!(selected)],
        ),
    );
    pump(&mut app, entity);
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].kind, "mcl");
    for (record, value) in records[1..].iter().zip(&selected) {
        let message = serde_json::from_value::<AgentMessage>(value.clone()).unwrap();
        let (content, payload) =
            margatroid_types::history_components::message_history_parts(&message);
        assert_eq!(record.content, content);
        assert_eq!(record.payload, payload);
        assert!(record.payload.get("content").is_none());
    }
    assert_eq!(*store.message_batches.lock().unwrap(), 1);
}

#[test]
fn duplicate_tool_completion_never_writes_messages() {
    use margatroid_types::block_types::BlockInner;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    let req = request(
        "tool",
        "a",
        "EMIT EFFECT tool_call ? ?",
        vec![json!([]), json!([])],
    );
    enqueue_mcl_command(app.world_mut(), req.clone());
    let pending = MclPending {
        command_id: "tool".into(),
        target_entity: entity,
        operation_id: "operation".into(),
        cancel: Default::default(),
    };
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .mcl
        .pending = Some(pending.clone());
    let output = MclAsyncOutput {
        request: req,
        pending,
        turn: None,
        result: Ok(MclAsyncPhase::Completed(MclValue::Array(
            BlockInner::Message(vec![AgentMessage::ToolCall {
                resource_id: None,
                tool_call_id: "failed".into(),
                tool_name: "run".into(),
                content: "failure".into(),
                failed: true,
            }]),
        ))),
    };
    complete_mcl_async(app.world_mut(), output.clone());
    complete_mcl_async(app.world_mut(), output);
    assert!(store.list().unwrap().is_empty());
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .drained
        .is_drained());
}

#[test]
fn import_alias_conflict_prechecks_before_loader_submission() {
    use margatroid_types::agent_components::{ImportedValue, WorldAgentExt};
    use margatroid_types::resource_events::LoadToolResource;
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    let first = callable("tool:test/one", "same");
    let second: ResourceID = "tool:test/two".parse().unwrap();
    let agent_id = {
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        agent.info.driver_dependencies = Arc::from([first.resource_id.clone(), second]);
        agent.resourceid.clone()
    };
    app.world_mut()
        .agent_bind_import(&agent_id, "same".into(), ImportedValue::Tool(first.clone()))
        .unwrap();
    app.world().send_event(request(
        "collision",
        "a",
        "IMPORT TOOL tool:test/two AS same",
        vec![],
    ));
    tick_until_result(&mut app, "collision");
    assert!(app
        .world()
        .event_reader::<LoadToolResource>()
        .into_iter()
        .next()
        .is_none());
    assert!(app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .any(|reply| reply.id == "collision" && reply.result == Err(MclError::AliasConflict)));
    assert_eq!(
        app.world().agent_get_import(&agent_id, "same").unwrap(),
        margatroid_types::block_types::BlockInner::Tool(vec![first])
    );
}

fn bind_tool(
    app: &mut App,
    entity: Entity,
    tool: margatroid_types::resource_components::CallableResource,
) {
    use margatroid_types::agent_components::{ImportedValue, WorldAgentExt};
    let id = {
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        agent.info.driver_dependencies = Arc::from([tool.resource_id.clone()]);
        agent.resourceid.clone()
    };
    app.world_mut()
        .agent_bind_import(&id, tool.alias.clone(), ImportedValue::Tool(tool))
        .unwrap();
}

#[test]
fn invalid_history_append_is_audited_without_message_records() {
    for (text, arguments) in [
        (
            "EMIT EFFECT history_append ?",
            vec![json!([{"type":"unknown"}])],
        ),
        (
            "EMIT EFFECT history_append ?",
            vec![json!([{"type":"inject","messages":[{"type":"system","content":"hidden"}]}])],
        ),
        ("EMIT EFFECT history_append (setting.visible)", vec![]),
    ] {
        let mut app = app();
        let (entity, store) = spawn(&mut app, "a");
        enqueue_mcl_command(
            app.world_mut(),
            request(
                "setting",
                "a",
                "CREATE BLOCK setting (visible TOOL)",
                vec![],
            ),
        );
        pump(&mut app, entity);
        store.records.lock().unwrap().clear();
        enqueue_mcl_command(
            app.world_mut(),
            request("invalid", "a", text, arguments.clone()),
        );
        pump(&mut app, entity);
        let records = store.list().unwrap();
        assert_eq!(records.len(), 1, "{text}");
        assert_eq!(records[0].kind, "mcl");
        assert_eq!(records[0].payload["text"], text);
        assert_eq!(records[0].payload["arguments"], json!(arguments));
        assert!(app
            .world()
            .get_component::<Agent>(entity)
            .unwrap()
            .mcl
            .drained
            .is_drained());
    }
}

#[test]
fn valid_history_append_store_failure_does_not_add_command_audit() {
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    *store.fail.lock().unwrap() = true;
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "append",
            "a",
            "EMIT EFFECT history_append ?",
            vec![json!([{"type":"user","content":"message"}])],
        ),
    );
    pump(&mut app, entity);
    assert!(store.records.lock().unwrap().is_empty());
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .drained
        .is_drained());
}

#[derive(Clone)]
struct GatedDatabase {
    database: memory_plugin::SqliteAgentDatabase,
    gate: Arc<(Mutex<(bool, bool)>, std::sync::Condvar)>,
    operations: Arc<Mutex<Vec<margatroid_types::block_storage::BlockStorageOperation>>>,
}
impl AgentHistoryStore for GatedDatabase {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        self.database.append_batch(records)
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        self.database.list()
    }
}
impl margatroid_types::block_storage::AgentBlockStorage for GatedDatabase {
    fn execute(
        &self,
        request: margatroid_types::block_storage::BlockStorageRequest,
    ) -> Result<
        margatroid_types::block_storage::StoredBlockResult,
        margatroid_types::block_storage::BlockStorageError,
    > {
        self.operations
            .lock()
            .unwrap()
            .push(request.operation.clone());
        let (lock, signal) = &*self.gate;
        let mut state = lock.lock().unwrap();
        state.0 = true;
        signal.notify_all();
        while !state.1 {
            state = signal.wait(state).unwrap();
        }
        drop(state);
        margatroid_types::block_storage::AgentBlockStorage::execute(&self.database, request)
    }
}

#[test]
fn database_cancel_keeps_other_activity_and_fifo_until_worker_really_joins() {
    use margatroid_types::block_storage::*;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    let backend = GatedDatabase {
        database: memory_plugin::SqliteAgentDatabase::open(":memory:").unwrap(),
        gate: Arc::new((Mutex::new((false, false)), std::sync::Condvar::new())),
        operations: Default::default(),
    };
    app.world_mut()
        .get_component_mut::<Agent>(entity)
        .unwrap()
        .database = AgentDatabaseHandle::new(backend.clone());
    enqueue_mcl_command(
        app.world_mut(),
        request("blocked", "a", "CREATE TEMP BLOCK b (m MESSAGE)", vec![]),
    );
    enqueue_mcl_command(
        app.world_mut(),
        request("tail", "a", "EMIT EFFECT start_turn", vec![]),
    );
    process_mcl_queue(app.world_mut(), entity);
    let (lock, signal) = &*backend.gate;
    let state = lock.lock().unwrap();
    let (state, timeout) = signal
        .wait_timeout_while(state, std::time::Duration::from_secs(3), |state| !state.0)
        .unwrap();
    assert!(!timeout.timed_out());
    drop(state);
    let scope = app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .clone();
    assert_eq!(scope.activity_count(), 1);
    scope.request_close();
    process_mcl_queue(app.world_mut(), entity);
    assert_eq!(scope.activity_count(), 1);
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert!(agent.mcl.pending.as_ref().unwrap().cancel.is_cancelled());
    assert!(!agent.mcl.drained.is_drained());
    assert!(agent.turn.is_none());
    lock.lock().unwrap().1 = true;
    signal.notify_all();
    tick_until_result(&mut app, "blocked");
    assert_eq!(scope.activity_count(), 0);
    assert!(app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .any(|result| result.id == "blocked" && result.result == Err(MclError::Cancelled)));
    pump(&mut app, entity);
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .drained
        .is_drained());
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn effect_selectors_use_one_database_request_and_current_import_ids() {
    use margatroid_types::agent_components::{ImportedValue, WorldAgentExt};
    use margatroid_types::block_storage::*;
    use margatroid_types::inference_events::InferenceRequest;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    let backend = GatedDatabase {
        database: memory_plugin::SqliteAgentDatabase::open(":memory:").unwrap(),
        gate: Arc::new((Mutex::new((false, true)), std::sync::Condvar::new())),
        operations: Default::default(),
    };
    let first = callable("tool:test/one", "old");
    let agent_id = {
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        agent.database = AgentDatabaseHandle::new(backend.clone());
        agent.info.driver_dependencies = Arc::from([first.resource_id.clone()]);
        agent.resourceid.clone()
    };
    app.world_mut()
        .agent_bind_import(&agent_id, "old".into(), ImportedValue::Tool(first.clone()))
        .unwrap();
    for (id, text, args) in [
        ("create", "CREATE BLOCK b (m MESSAGE, t TOOL)", vec![]),
        (
            "message",
            "INJECT ? TO b.m",
            vec![json!([{"type":"inject","messages":[{"type":"user","content":"preserve"}]}])],
        ),
        ("tools", "INJECT old TO b.t", vec![]),
    ] {
        enqueue_mcl_command(app.world_mut(), request(id, "a", text, args));
    }
    pump(&mut app, entity);
    let mut current = first.clone();
    current.alias = "current".into();
    app.world_mut()
        .agent_bind_import(
            &agent_id,
            "current".into(),
            ImportedValue::Tool(current.clone()),
        )
        .unwrap();
    backend.operations.lock().unwrap().clear();
    let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
    agent.new_turn();
    agent.state = AgentState::Working;
    enqueue_mcl_command(
        app.world_mut(),
        request("infer", "a", "EMIT EFFECT inference (b.m) (b.t)", vec![]),
    );
    pump(&mut app, entity);
    let requests = backend.operations.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let BlockStorageOperation::Get { expressions } = &requests[0] else {
        panic!()
    };
    assert_eq!(expressions.len(), 2);
    drop(requests);
    let inference = app
        .world()
        .event_reader::<InferenceRequest>()
        .into_iter()
        .next()
        .unwrap()
        .clone();
    assert_eq!(inference.tools, vec![current]);
    assert!(
        matches!(&inference.messages[..],[AgentMessage::Inject {messages}] if messages.len()==1)
    );
    assert_eq!(store.list().unwrap().len(), 4);
    inference
        .reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(InferenceResponse {
            message: AgentMessage::Assistant {
                reasoning: None,
                content: None,
                tool_calls: vec![],
                usage: None,
                stop_reason: None,
            },
            stop_reason: StopReason::Completed,
        }))
        .unwrap();
    tick_until_result(&mut app, "infer");
    assert_eq!(store.list().unwrap().len(), 4);
}

#[test]
fn create_returns_opened_and_failed_inject_preserves_stored_messages() {
    use margatroid_types::block_storage::StoredBlockInner;
    use margatroid_types::block_types::BlockPath;
    let mut app = app();
    let (entity, store) = spawn(&mut app, "a");
    for (id, text, created) in [
        ("first", "CREATE TEMP BLOCK b (m MESSAGE)", true),
        ("reopen", "CREATE TEMP BLOCK b (m MESSAGE)", false),
    ] {
        app.world().send_event(request(id, "a", text, vec![]));
        tick_until_result(&mut app, id);
        assert!(app
            .world()
            .event_reader::<MclCommandResult>()
            .into_iter()
            .any(|reply| reply.id == id
                && reply.result
                    == Ok(MclValue::BlockOpened {
                        block_id: "b".into(),
                        created,
                        temporary: true
                    })));
    }
    enqueue_mcl_command(
        app.world_mut(),
        request(
            "inject",
            "a",
            "INJECT ? TO b.m",
            vec![json!([{"type":"inject","messages":[{"type":"user","content":"kept"}]}])],
        ),
    );
    pump(&mut app, entity);
    let path = BlockPath {
        block_id: "b".into(),
        inner_id: "m".into(),
    };
    let before = read_stored(&app, entity, path.clone());
    assert!(
        matches!(&before,StoredBlockInner::Message(messages) if matches!(&messages[..],[AgentMessage::Inject {..}]))
    );
    app.world().send_event(request(
        "invalid",
        "a",
        "INJECT b.m[0], b.m[999] TO b.m",
        vec![],
    ));
    tick_until_result(&mut app, "invalid");
    assert_eq!(read_stored(&app, entity, path), before);
    assert!(app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .any(|reply| reply.id == "invalid" && reply.result == Err(MclError::InvalidRange)));
    assert_eq!(store.list().unwrap().len(), 4);
    assert_eq!(*store.message_batches.lock().unwrap(), 0);
}

#[derive(Clone)]
struct FailingDatabase {
    history: bool,
}
impl AgentHistoryStore for FailingDatabase {
    fn append_batch(&self, _: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        if self.history {
            Err(HistoryError::CleanupFailed(
                "unresolved audit rollback".into(),
            ))
        } else {
            Ok(())
        }
    }
    fn list(&self) -> Result<Vec<HistoryRecord>, HistoryError> {
        Ok(vec![])
    }
}
impl margatroid_types::block_storage::AgentBlockStorage for FailingDatabase {
    fn execute(
        &self,
        _: margatroid_types::block_storage::BlockStorageRequest,
    ) -> Result<
        margatroid_types::block_storage::StoredBlockResult,
        margatroid_types::block_storage::BlockStorageError,
    > {
        Err(
            margatroid_types::block_storage::BlockStorageError::CleanupFailed(
                "unresolved block rollback".into(),
            ),
        )
    }
}

#[test]
fn unresolved_block_or_audit_cleanup_retains_the_execution_activity() {
    use margatroid_types::block_storage::AgentDatabaseHandle;
    use margatroid_types::execution_components::ExecutionError;
    for history in [false, true] {
        let mut app = app();
        let (entity, _) = spawn(&mut app, "a");
        let database = AgentDatabaseHandle::new(FailingDatabase { history });
        {
            let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
            agent.history = database.history();
            agent.database = database;
        }
        app.world()
            .send_event(request("fail", "a", "CREATE BLOCK b (m MESSAGE)", vec![]));
        tick_until_result(&mut app, "fail");
        assert!(app
            .world()
            .event_reader::<MclCommandResult>()
            .into_iter()
            .any(|reply| reply.id == "fail"
                && matches!(reply.result, Err(MclError::StorageCleanupFailed(_)))));
        let agent = app.world().get_component::<Agent>(entity).unwrap();
        assert_eq!(agent.execution.activity_count(), 1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(matches!(
            runtime.block_on(agent.execution.wait_all()),
            Err(ExecutionError::CleanupFailed { .. })
        ));
        assert!(agent.mcl.pending.is_none());
        assert!(!agent.mcl.drained.is_drained());
        assert!(agent.execution.is_closed());
        assert!(matches!(
            runtime.block_on(agent.mcl.drained.wait_drained()),
            Err(ExecutionError::CleanupFailed { .. })
        ));
    }
}

#[test]
fn cancelled_audit_cleanup_failure_reports_receipt_but_fails_drain() {
    use margatroid_types::block_storage::AgentDatabaseHandle;
    use margatroid_types::execution_components::ExecutionError;
    let mut app = app();
    let (entity, _) = spawn(&mut app, "a");
    let database = AgentDatabaseHandle::new(FailingDatabase { history: true });
    {
        let agent = app.world_mut().get_component_mut::<Agent>(entity).unwrap();
        agent.history = database.history();
        agent.database = database;
    }
    enqueue_mcl_command(
        app.world_mut(),
        request("cancelled", "a", "EMIT EFFECT start_turn", vec![]),
    );
    app.world()
        .get_component::<Agent>(entity)
        .unwrap()
        .execution
        .request_close();
    tick_until_result(&mut app, "cancelled");
    assert!(app
        .world()
        .event_reader::<MclCommandResult>()
        .into_iter()
        .any(|reply| reply.id == "cancelled"
            && reply.result == Err(MclError::Cancelled)
            && matches!(reply.history_error, Some(HistoryError::CleanupFailed(_)))));
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert_eq!(agent.mcl.cancel_audits, 0);
    assert!(agent.mcl.commands.is_empty());
    assert!(!agent.mcl.drained.is_drained());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert!(matches!(
        runtime.block_on(agent.mcl.drained.wait_drained()),
        Err(ExecutionError::CleanupFailed { .. })
    ));
}
