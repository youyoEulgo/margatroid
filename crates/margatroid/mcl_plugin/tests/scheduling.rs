use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, AgentInfo, AgentState};
use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};
use margatroid_types::lua_runtime_components::{DriverLuaVmInstance, DriverReadySlot};
use margatroid_types::message_types::{AgentMessage, InferenceResponse, StopReason};
use margatroid_types::resource_components::AgentImageModelConfig;
use mcl_plugin::*;
use resource_id_plugin::{ResourceID, ResourcePlugin, WorldResourceExt};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Store {
    records: Arc<Mutex<Vec<HistoryRecord>>>,
    fail: Arc<Mutex<bool>>,
}
impl AgentHistoryStore for Store {
    fn append_batch(&self, records: Vec<HistoryRecord>) -> Result<(), HistoryError> {
        if *self.fail.lock().unwrap() {
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
                    model: AgentImageModelConfig {
                        model: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                        top_p: None,
                        stop: vec![],
                    },
                    project_root: Default::default(),
                    home_root: Default::default(),
                    image: "agent_image:test/demo".parse().unwrap(),
                    image_dependencies: Arc::from([]),
                },
                workspace: "workspace:test/demo".parse().unwrap(),
                sandbox: None,
                lua: Default::default(),
                state: AgentState::Running,
                history: AgentHistoryHandle::new(store.clone()),
                block: Default::default(),
                import_bindings: Default::default(),
                mcl: Default::default(),
                execution: Default::default(),
                turn: None,
            },
        )
        .unwrap();
    (entity, store)
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
    process_mcl_queue(app.world_mut(), entity);
    let agent = app.world().get_component::<Agent>(entity).unwrap();
    assert!(agent.turn.is_some());
    assert!(agent.mcl.commands.is_empty());
    assert!(agent.mcl.drained.is_drained());
    assert_eq!(store.list().unwrap().len(), 3);
    *store.fail.lock().unwrap() = true;
    enqueue_mcl_command(
        app.world_mut(),
        request("4", "a", "EMIT EFFECT finish", vec![]),
    );
    process_mcl_queue(app.world_mut(), entity);
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
            request(&i.to_string(), "a", "EMIT EFFECT finish", vec![]),
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
    process_mcl_queue(app.world_mut(), a);
    process_mcl_queue(app.world_mut(), b);
    assert_eq!(
        app.world()
            .get_component::<Agent>(a)
            .unwrap()
            .mcl
            .commands
            .len(),
        1
    );
    assert!(app
        .world()
        .get_component::<Agent>(b)
        .unwrap()
        .turn
        .is_some());
    process_mcl_queue(app.world_mut(), a);
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
        request("tail", "a", "EMIT EFFECT finish", vec![]),
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
    process_mcl_queue(app.world_mut(), a);
    assert_eq!(store.list().unwrap().len(), 0);
    app.world()
        .get_component::<Agent>(a)
        .unwrap()
        .execution
        .request_close();
    process_mcl_queue(app.world_mut(), a);
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
        result: Ok(MclValue::Inference(InferenceResponse {
            message: AgentMessage::Assistant {
                reasoning: None,
                content: Some("done".into()),
                tool_calls: vec![],
                usage: None,
            },
            stop_reason: StopReason::Completed,
        })),
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
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].turn.as_deref(), Some("original"));
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
    process_mcl_queue(app.world_mut(), a);
    let mut first = driver_call(&app, a, "a", "EMIT EFFECT ready");
    let mut second = driver_call(&app, a, "a", "EMIT EFFECT ready");
    assert!(poll_host(&mut first).is_pending());
    assert!(poll_host(&mut second).is_pending());
    for _ in 0..3 {
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
    app.world_mut().get_component_mut::<Agent>(a).unwrap().turn = Some("submitted".into());
    app.world().send_event(request(
        "infer",
        "a",
        "EMIT EFFECT inference ? ?",
        vec![json!([]), json!([])],
    ));
    app.world()
        .send_event(request("tail", "a", "EMIT EFFECT finish", vec![]));
    let mut inference = None;
    for _ in 0..4 {
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
            },
            stop_reason: StopReason::Completed,
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
    assert!(app
        .world()
        .get_component::<Agent>(a)
        .unwrap()
        .turn
        .is_none());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].turn.as_deref(), Some("submitted"));
    assert_eq!(records[1].payload["content"], "actual");
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
    process_mcl_queue(app.world_mut(), a);
    let agent = app.world().get_component::<Agent>(a).unwrap();
    assert!(agent.mcl.pending.is_none());
    assert!(agent.turn.is_some());
    let records = store.list().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].payload["tool_call_id"], "one");
    assert_eq!(records[1].payload["failed"], true);
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
    process_mcl_queue(app.world_mut(), a);
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
        .image_dependencies = Arc::from([id]);
    app.world().send_event(request(
        "load",
        "a",
        "IMPORT TOOL tool:test/one AS named",
        vec![],
    ));
    app.world()
        .send_event(request("tail", "a", "EMIT EFFECT wake", vec![]));
    let mut load = None;
    for _ in 0..4 {
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
    assert_eq!(store.list().unwrap().len(), 2);
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
    for _ in 0..3 {
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
    process_mcl_queue(app.world_mut(), b);
    assert!(ready_rx.try_recv().is_err());
    let mut own = driver_call(&app, b, "b", "EMIT EFFECT ready");
    assert!(poll_host(&mut own).is_pending());
    let mut observed = false;
    for _ in 0..3 {
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
    for _ in 0..3 {
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
    for _ in 0..3 {
        app.tick();
    }
    assert!(
        matches!(poll_host(&mut stale), std::task::Poll::Ready(Err(_))),
        "old source instance cannot ready replacement"
    );
}

#[test]
fn successful_async_result_survives_history_error() {
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
    *store.fail.lock().unwrap() = true;
    complete_mcl_async(
        app.world_mut(),
        MclAsyncOutput {
            request: req.clone(),
            pending,
            turn: None,
            result: Ok(MclValue::Inference(InferenceResponse {
                message: AgentMessage::Assistant {
                    reasoning: None,
                    content: Some("real".into()),
                    tool_calls: vec![],
                    usage: None,
                },
                stop_reason: StopReason::Length,
            })),
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
    assert!(reply.history_error.is_some());
    assert!(
        matches!(&reply.result,Ok(MclValue::Inference(response)) if response.stop_reason==StopReason::Length)
    );
}

#[test]
fn real_tool_batch_keeps_order_and_records_failed_items() {
    use margatroid_types::resource_components::ToolResource;
    use margatroid_types::tool_events::{
        ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse,
    };
    use resource_id_plugin::ResourceSet;
    let mut app = app();
    let (a, store) = spawn(&mut app, "a");
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
    for _ in 0..4 {
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
    assert_eq!(records.len(), 4);
    assert_eq!(records[1].payload["tool_call_id"], "first");
    assert_eq!(records[2].payload["tool_call_id"], "second");
    assert_eq!(records[3].payload["tool_call_id"], "third");
    assert_eq!(records[2].payload["failed"], true);
    assert_eq!(records[3].payload["failed"], true);
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
        agent.info.image_dependencies = Arc::from([id.clone()]);
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
    process_mcl_queue(app.world_mut(), a);
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
    process_mcl_queue(app.world_mut(), a);
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
    process_mcl_queue(app.world_mut(), a);
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
    process_mcl_queue(app.world_mut(), a);
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
        agent.info.image_dependencies = Arc::from([prompt.clone()]);
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
    process_mcl_queue(app.world_mut(), entity);
    let path = BlockPath {
        block_id: "msg".into(),
        inner_id: "ctx".into(),
    };
    let old = BlockInner::Message(vec![AgentMessage::System {
        content: "old".into(),
    }]);
    assert_eq!(
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .block
            .get(&path)
            .unwrap(),
        &old
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
        app.world()
            .get_component::<Agent>(entity)
            .unwrap()
            .block
            .get(&path)
            .unwrap(),
        &old
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
    app.tick();
    app.tick();
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
    assert_eq!(store.list().unwrap().len(), 1);
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
        agent.info.image_dependencies = Arc::from([first.resource_id.clone(), second]);
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
    app.tick();
    app.tick();
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
        agent.info.image_dependencies = Arc::from([tool.resource_id.clone()]);
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
        app.world_mut()
            .get_component_mut::<Agent>(entity)
            .unwrap()
            .block
            .create(
                "setting".into(),
                vec![(
                    "visible".into(),
                    margatroid_types::block_types::InnerType::Tool,
                )],
            )
            .unwrap();
        enqueue_mcl_command(
            app.world_mut(),
            request("invalid", "a", text, arguments.clone()),
        );
        process_mcl_queue(app.world_mut(), entity);
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
    process_mcl_queue(app.world_mut(), entity);
    assert!(store.records.lock().unwrap().is_empty());
    assert!(app
        .world()
        .get_component::<Agent>(entity)
        .unwrap()
        .mcl
        .drained
        .is_drained());
}
