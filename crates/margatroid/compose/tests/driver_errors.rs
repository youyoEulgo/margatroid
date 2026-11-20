use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::{App, World};
use lua_runtime_core::*;
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_core::{MclCommand, MclValue, MessageArray};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use workspace_core::{WorkspaceExecutionClose, WorkspaceState, spawn_workspace_executor};

const CODER: &str =
    include_str!("../../../../resources/agent-drivers/margatroid/coder/dev/base.lua");
const REVIEWER: &str =
    include_str!("../../../../resources/agent-drivers/margatroid/reviewer/dev/base.lua");

#[derive(Default)]
struct Commands {
    pending: VecDeque<Value>,
    history: Vec<Value>,
    calls: Vec<String>,
    fail: Option<String>,
    recovery_fail: Option<String>,
    recovery_after_failure: Option<String>,
    working: bool,
    finished: usize,
    malformed_config: bool,
    initialization: Option<Value>,
    inference_error: bool,
    conversation: Vec<Value>,
    snapshots: Vec<Vec<Value>>,
}

// Run the shipped drivers in a real Workspace Lua VM. The command responder is
// deliberately controlled so every boundary can fail independently of storage.
struct Fixture {
    app: App,
    _root: tempfile::TempDir,
    owner: core_plugin::Entity,
    workspace: core_plugin::Entity,
    close: WorkspaceExecutionClose,
    commands: Arc<Mutex<Commands>>,
    exits: Arc<Mutex<Vec<DriverLuaVmExited>>>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(LuaRuntimePlugin::new());
        let owner = app.world_mut().spawn();
        app.world_mut().insert_component(
            owner,
            address::Address::new("agent:margatroid/driver-errors:dev").unwrap(),
        );
        let storage = storage_core::open_storage(
            &root.path().join("agent.sqlite"),
            &root.path().join("settings.toml"),
            "[compaction]\nthreshold = 0.8\n",
        )
        .unwrap();
        app.world_mut().insert_component(owner, storage);
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            address::Address::new("workspace:test/unit:dev").unwrap(),
        );
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new(
                "driver-errors".into(),
                root.path().join("workspace.yaml"),
                root.path().to_owned(),
                WorkspaceState::Running,
                Default::default(),
                None,
                crate::test_workspace_context(),
            ),
        );
        let executor = spawn_workspace_executor().unwrap();
        let close = executor.close.clone();
        app.world_mut().insert_component(workspace, executor);
        let events = app.world().event_sender();
        let closures = app.world().closure_sender(RuntimePlugin::UPDATE);
        app.world_mut().insert_resource(LuaServices::new(move|identity,method,args|{let events=events.clone();let closures=closures.clone();Box::pin(async move{
            match method.as_str(){
                "execution_finished"|"tools_register"=>Ok(json!({})),
                "agent_create"=>Ok(json!({"address":"agent:margatroid/driver-errors:dev"})),
                "wait"=>{let(tx,rx)=oneshot::channel();closures.send(move|world|{let _=tx.send(world.get_component::<DriverLuaVm>(owner).unwrap().wake.clone());});let slot=rx.await.unwrap();tokio::select!{_=identity.cancel.cancelled()=>Err(LuaRuntimeError::new(LuaRuntimeErrorKind::Cancelled,"cancelled")),result=DriverWakeSlot::wait(slot)=>{result?;Ok(serde_json::Value::Null)}}},
                "task_receive"=>std::future::pending::<Result<Value,LuaRuntimeError>>().await,
                "mcl_execute"=>{let(tx,rx)=oneshot::channel();events.send_event(MclCommand{id:"fixture-mcl".into(),source:identity.source,source_address:identity.address,source_instance:Some(identity.instance),target:owner,instruction:mcl_core::MclInstruction{text:args["instruction"].as_str().unwrap().into(),arguments:serde_json::from_value(args["arguments"].clone()).unwrap()},cancel:identity.cancel,reply:Arc::new(Mutex::new(Some(tx)))});Ok(match rx.await.unwrap().unwrap(){MclValue::Unit=>json!({}),value=>serde_json::to_value(value).unwrap()})},
                _=>Err(LuaRuntimeError::new(LuaRuntimeErrorKind::InvalidRequest,"unknown service")),
            }
        })}));
        let commands = Arc::new(Mutex::new(Commands::default()));
        let state = commands.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            let requests: Vec<_> = world.event_reader::<MclCommand>().into_iter().cloned().collect();
            for request in requests {
                let text = request.instruction.text.as_str();
                let mut state = state.lock().unwrap();
                state.calls.push(text.to_owned());
                let failed = state.fail.as_deref() == Some(text) || state.recovery_fail.as_deref() == Some(text);
                let result = if failed {
                    if state.fail.as_deref() == Some(text) {
                        state.fail = None;
                        state.recovery_fail = state.recovery_after_failure.take();
                    }
                    if state.recovery_fail.as_deref() == Some(text) { state.recovery_fail = None; }
                    MclValue::Error(json!({"type":"error", "message":format!("injected {text}"), "request_id":request.id}))
                } else if text == "CONFIG GET" {
                    if state.malformed_config {
                        state.malformed_config = false;
                        MclValue::Unit
                    } else {
                        let mut config:toml::Table=toml::from_str("sandbox='read_only'\n[compaction]\nthreshold=0.8\nkeep_messages=2\n").unwrap();
                        let initialization=state.initialization.clone().unwrap_or_else(||toml::from_str::<Value>(include_str!("../../../../resources/agent-drivers/margatroid/coder/dev/setting.toml")).unwrap()["initialization"].clone());
                        config.insert("initialization".into(),toml::Value::try_from(initialization).unwrap());
                        MclValue::Config(config)
                    }
                } else if text=="EFFECT model_set ?"{MclValue::Config(toml::from_str("context_window_tokens=4096").unwrap())
                } else if text == "GET session.conversation" {
                    MclValue::Array(MessageArray::Message(state.conversation.clone()))
                } else if text == "EFFECT block_snapshot ?" {
                    assert_eq!(request.instruction.arguments, vec![json!("session")]);
                    let saved = state.conversation.clone();
                    state.snapshots.push(saved);
                    MclValue::Unit
                } else if text == "INJECT ? TO session.conversation" {
                    state.conversation = request.instruction.arguments[0].as_array().unwrap().clone();
                    MclValue::Unit
                } else if text == "GET entry.pending[0]" {
                    MclValue::Array(MessageArray::Message(state.pending.front().cloned().into_iter().collect()))
                } else if text == "INJECT [] TO entry.pending[0,1]" {
                    state.pending.pop_front();
                    MclValue::Unit
                } else if text == "INJECT ? TO entry.pending[-1]" {
                    state.pending.extend(request.instruction.arguments[0].as_array().unwrap().iter().cloned());
                    MclValue::Unit
                } else if text == "EFFECT history_append ?" {
                    let mut recorded = Vec::new();
                    for mut message in request.instruction.arguments[0].as_array().unwrap().iter().cloned() {
                        if message.get("sequence").is_none() {
                            message["sequence"] = json!(state.history.len() + 1);
                            state.history.push(message.clone());
                        }
                        recorded.push(message);
                    }
                    MclValue::Array(MessageArray::Message(recorded))
                } else if text.starts_with("EFFECT inference ") && std::mem::take(&mut state.inference_error) {
                    MclValue::Inference{message:json!({"type":"error","message":"provider connection refused","request_id":request.id}),stop_reason:"error".into()}
                } else if text.starts_with("EFFECT inference ") {
                    MclValue::Inference {message:json!({"type":"assistant", "content":"done", "stop_reason":"completed"}), stop_reason:"completed".into()}
                } else if text == "EFFECT start_turn" {
                    state.working = true;
                    MclValue::Turn("driver-error-turn".into())
                } else if text == "EFFECT finish_turn" {
                    state.working = false;
                    state.finished += 1;
                    MclValue::Unit
                } else {
                    MclValue::Unit
                };
                drop(state);
                if text == "EFFECT ready" && !failed {
                    world.driver_ready(&DriverLuaVmInstance {owner:request.source, start_id:request.source_instance.clone().unwrap()}).unwrap();
                }
                if let Some(reply) = request.reply.lock().unwrap().take() {
                    let _ = reply.send(Ok(result));
                }
            }
        });
        let exits = Arc::new(Mutex::new(Vec::new()));
        let records = exits.clone();
        app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            records.lock().unwrap().extend(
                world
                    .event_reader::<DriverLuaVmExited>()
                    .into_iter()
                    .cloned(),
            );
        });
        Self {
            app,
            _root: root,
            owner,
            workspace,
            close,
            commands,
            exits,
        }
    }
    async fn start(&mut self, source: &str) -> Result<DriverLuaVmInstance, LuaRuntimeError> {
        let (sender, mut reply) = oneshot::channel();
        let source = if source.contains("function configure(") {
            source.to_owned()
        } else {
            format!(
                "function configure()return {{timeout_ms=0}}end\nfunction execute()\n{source}\nend"
            )
        };
        self.app.world().send_event(StartDriverLuaVm {
            owner:self.owner, workspace:self.workspace,
            program:LuaProgram {source, origin:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../resources/agent-drivers/margatroid/coder/dev/shipped-driver.lua"), },
            environment:LuaEnvironment {globals:BTreeMap::from([("execution_context".into(), LuaBinding::Value(LuaValue::from_json(json!({"workspace":{"address":"workspace:test/unit:dev"},"name":"test","resource_dir":"/tmp"})).unwrap()))]), modules:BTreeMap::new()},
            reply:Arc::new(Mutex::new(Some(sender))),
        });
        self.drive(|| reply.try_recv().ok()).await
    }
    async fn drive<T>(&mut self, mut result: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            if let Some(result) = result() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "driver failed to settle: {:?}",
                self.commands.lock().unwrap().calls
            );
            tokio::task::yield_now().await;
        }
    }
    async fn settle(&mut self) {
        // Allow the async continuation to reach wait, without assuming a fixed
        // number of commands or scheduling order after the last reply.
        for _ in 0..200 {
            self.app.tick();
            tokio::task::yield_now().await;
        }
    }
    fn wake(&mut self, instance: &DriverLuaVmInstance) {
        self.commands
            .lock()
            .unwrap()
            .pending
            .push_back(json!({"type":"user", "content":"next message"}));
        self.app.world().driver_wake(instance).unwrap();
    }
    async fn close(mut self, instance: Option<DriverLuaVmInstance>) {
        if let Some(instance) = instance {
            let (sender, mut reply) = oneshot::channel();
            self.app.world().send_event(StopDriverLuaVm {
                instance,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            self.drive(|| reply.try_recv().ok()).await.unwrap();
        }
        self.close.close_and_join().await.unwrap();
    }
}

#[tokio::test]
async fn shipped_drivers_stop_at_each_mcl_failure_and_host_logs_once() {
    for source in [CODER, REVIEWER] {
        for stage in [
            "GET entry.pending[0]",
            "INJECT [] TO entry.pending[0,1]",
            "EFFECT history_append ?",
            "INJECT ? TO session.conversation[-1]",
            "EFFECT start_turn",
            "CONFIG GET",
            "EFFECT inference (msg.instructions, session.conversation) ?",
            "INJECT ? TO entry.pending[-1]",
            "EFFECT finish_turn",
        ] {
            let mut fixture = Fixture::new();
            let instance = fixture.start(source).await.unwrap();
            fixture.settle().await;
            fixture.commands.lock().unwrap().fail = Some(stage.into());
            fixture.wake(&instance);
            let exits = fixture.exits.clone();
            fixture
                .drive(|| (!exits.lock().unwrap().is_empty()).then_some(()))
                .await;
            assert!(fixture.exits.lock().unwrap()[0].result.is_err());
            assert!(fixture.commands.lock().unwrap().fail.is_none());
            assert!(
                !fixture
                    .commands
                    .lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|item| item["type"] == "error")
            );
            let count = fixture.commands.lock().unwrap().calls.len();
            fixture.settle().await;
            assert_eq!(
                fixture.commands.lock().unwrap().calls.len(),
                count,
                "script continued after {stage}"
            );
            let logs = storage_core::read_agent_logs(
                fixture
                    .app
                    .world()
                    .get_component::<storage_core::AgentStorage>(fixture.owner)
                    .unwrap(),
                None,
                100,
            )
            .unwrap();
            assert_eq!(logs.len(), 1);
            assert!(
                logs[0]
                    .record
                    .raw_error
                    .as_deref()
                    .unwrap()
                    .contains(&format!("injected {stage}"))
            );
            fixture.close(Some(instance)).await;
        }
    }
}

#[tokio::test]
async fn lua_programming_errors_are_logged_by_host_and_exit_without_script_recovery() {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        let instance = fixture.start(source).await.unwrap();
        fixture.settle().await;
        fixture.commands.lock().unwrap().malformed_config = true;
        fixture.wake(&instance);
        let exits = fixture.exits.clone();
        fixture
            .drive(|| (!exits.lock().unwrap().is_empty()).then_some(()))
            .await;
        assert!(fixture.exits.lock().unwrap()[0].result.is_err());
        assert!(
            !fixture
                .commands
                .lock()
                .unwrap()
                .history
                .iter()
                .any(|message| message["type"] == "error"),
            "Lua programming error was converted into conversation data"
        );
        let logs = storage_core::read_agent_logs(
            fixture
                .app
                .world()
                .get_component::<storage_core::AgentStorage>(fixture.owner)
                .unwrap(),
            None,
            100,
        )
        .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].record.source, storage_core::AgentLogSource::Driver);
        assert!(
            logs[0]
                .record
                .raw_error
                .as_deref()
                .unwrap()
                .contains("config")
        );
        assert!(
            logs[0]
                .record
                .traceback
                .as_deref()
                .is_some_and(|trace| trace.contains("shipped-driver.lua"))
        );
        let before = fixture.commands.lock().unwrap().calls.len();
        fixture.settle().await;
        assert_eq!(fixture.commands.lock().unwrap().calls.len(), before);
        fixture.close(Some(instance)).await;
    }
}

#[tokio::test]
async fn initialization_mcl_error_exits_through_host_without_ready() {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        fixture.commands.lock().unwrap().fail = Some("CONFIG GET".into());
        let failure = fixture.start(source).await.unwrap_err();
        assert!(failure.to_string().contains("injected CONFIG GET"));
        assert!(
            !fixture
                .commands
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|call| call == "EFFECT ready")
        );
        let logs = storage_core::read_agent_logs(
            fixture
                .app
                .world()
                .get_component::<storage_core::AgentStorage>(fixture.owner)
                .unwrap(),
            None,
            100,
        )
        .unwrap();
        assert_eq!(logs.len(), 1);
        assert!(
            logs[0]
                .record
                .raw_error
                .as_deref()
                .unwrap()
                .contains("injected CONFIG GET")
        );
        fixture.close(None).await;
    }
}

#[tokio::test]
async fn compaction_snapshots_before_replacement_and_preserves_context_on_snapshot_failure() {
    for source in [CODER, REVIEWER] {
        for (tokens, fail_snapshot) in [(4000, false), (4000, true), (0, false)] {
            let mut fixture = Fixture::new();
            let original = vec![
                json!({"type":"user","content":"old question"}),
                json!({"type":"assistant","content":"old answer"}),
                json!({"type":"user","content":"recent question"}),
                json!({"type":"assistant","content":"recent answer"}),
            ];
            {
                let mut state = fixture.commands.lock().unwrap();
                state.conversation = original.clone();
                if fail_snapshot {
                    state.fail = Some("EFFECT block_snapshot ?".into());
                }
            }
            let prefix = source.split("\n    initialize()\n").next().unwrap();
            let program = format!(
                r#"{prefix}
active_model = {{context_window_tokens=4096}}
local before = #command("GET session.conversation").Array.Message
assert(maybe_compact({{}}) == false)
local needed = maybe_compact({{usage = {{input_tokens = {tokens}, output_tokens = 0}}}})
assert(type(needed) == "boolean")
assert(needed == ({tokens} >= 4096 * 0.8))
assert(#command("GET session.conversation").Array.Message == before)
if needed then
    local result = compact()
    assert(result.type == "compact" and result.content == "done")
    assert(#command("GET session.conversation").Array.Message == before)
    process_compaction(result)
end
local marker = {{type = "user", content = "probe-ok"}}
command("EFFECT history_append ?", {{{{marker}}}})
active_model = {{context_window_tokens=4096}}
command("EFFECT ready")
margatroid:wait(agent.address)
end
"#
            );
            let started = fixture.start(&program).await;
            if fail_snapshot {
                assert!(
                    started
                        .unwrap_err()
                        .to_string()
                        .contains("injected EFFECT block_snapshot")
                );
                {
                    let state = fixture.commands.lock().unwrap();
                    assert_eq!(state.conversation, original);
                    assert!(
                        !state
                            .calls
                            .iter()
                            .any(|c| c == "INJECT soul TO msg.instructions")
                    );
                    assert!(
                        !state
                            .calls
                            .iter()
                            .any(|call| call == "INJECT ? TO session.conversation")
                    );
                }
                fixture.close(None).await;
                continue;
            }
            let instance = started.unwrap();
            {
                let state = fixture.commands.lock().unwrap();
                let expected = if fail_snapshot {
                    "probe-failed"
                } else {
                    "probe-ok"
                };
                assert_eq!(state.history.last().unwrap()["content"], expected);
                if tokens == 0 || fail_snapshot {
                    assert_eq!(state.conversation, original);
                    assert!(state.snapshots.is_empty());
                    assert!(
                        !state
                            .calls
                            .iter()
                            .any(|c| c == "INJECT ? TO session.conversation")
                    );
                } else {
                    assert!(state.history.iter().any(
                        |message| message["type"] == "compact" && message["content"] == "done"
                    ));
                    assert_eq!(state.snapshots, vec![original.clone()]);
                    assert_eq!(state.conversation.len(), 3);
                    assert_eq!(
                        state.conversation[0]["content"],
                        "Conversation checkpoint:\ndone"
                    );
                    assert_eq!(state.conversation[1..], original[2..]);
                    let snap = state
                        .calls
                        .iter()
                        .position(|c| c == "EFFECT block_snapshot ?")
                        .unwrap();
                    let replace = state
                        .calls
                        .iter()
                        .position(|c| c == "INJECT ? TO session.conversation")
                        .unwrap();
                    let refresh_compact = state
                        .calls
                        .iter()
                        .position(|c| c == "INJECT compact TO msg.compact_prompt")
                        .unwrap();
                    let inference = state
                        .calls
                        .iter()
                        .position(|c| c.starts_with("EFFECT inference "))
                        .unwrap();
                    let refresh_soul = state
                        .calls
                        .iter()
                        .position(|c| c == "INJECT soul TO msg.instructions")
                        .unwrap();
                    assert!(refresh_compact < inference);
                    assert!(snap < replace && replace < refresh_soul);
                }
            }
            fixture.close(Some(instance)).await;
        }
    }
}

#[tokio::test]
async fn compact_returns_error_or_compact_without_history_or_context_updates_until_caller_records_it()
 {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        let original = vec![
            json!({"type":"user","content":"old"}),
            json!({"type":"assistant","content":"answer"}),
            json!({"type":"user","content":"recent"}),
            json!({"type":"assistant","content":"last"}),
        ];
        {
            let mut state = fixture.commands.lock().unwrap();
            state.conversation = original.clone();
            state.inference_error = true;
        }
        let prefix = source.split("\n    initialize()\n").next().unwrap();
        let program = format!(
            r#"{prefix}
active_model={{context_window_tokens=4096}}
local failed=compact()
assert(failed.type=="error" and failed.message=="provider connection refused")
assert(#command("GET session.conversation").Array.Message==4)
process_compaction(failed)
assert(#command("GET session.conversation").Array.Message==4)
command("EFFECT ready")
margatroid:wait(agent.address)
end
"#
        );
        let instance = fixture.start(&program).await.unwrap();
        {
            let state = fixture.commands.lock().unwrap();
            assert_eq!(state.conversation, original);
            assert!(state.snapshots.is_empty());
            assert_eq!(state.history.len(), 1);
            assert_eq!(state.history[0]["type"], "error");
            assert_eq!(state.history[0]["message"], "provider connection refused");
            assert!(
                !state
                    .calls
                    .iter()
                    .any(|c| c == "INJECT soul TO msg.instructions")
            );
            let refresh = state
                .calls
                .iter()
                .position(|c| c == "INJECT compact TO msg.compact_prompt")
                .unwrap();
            let inference = state
                .calls
                .iter()
                .position(|c| c.starts_with("EFFECT inference "))
                .unwrap();
            assert!(refresh < inference);
            assert!(
                !state
                    .calls
                    .iter()
                    .any(|call| call == "INJECT ? TO session.conversation")
            );
        }
        fixture.close(Some(instance)).await;
    }
}

#[tokio::test]
async fn manual_compaction_without_earlier_context_returns_and_records_error_without_inference() {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        let prefix = source.split("\n    initialize()\n").next().unwrap();
        let program = format!(
            r#"{prefix}
active_model={{context_window_tokens=4096}}
local result=compact()
assert(result.type=="error" and result.message=="No earlier context to compact")
process_compaction(result)
command("EFFECT ready")
margatroid:wait(agent.address)
end
"#
        );
        let instance = fixture.start(&program).await.unwrap();
        {
            let state = fixture.commands.lock().unwrap();
            assert_eq!(state.history.len(), 1);
            assert_eq!(state.history[0]["type"], "error");
            assert!(state.conversation.is_empty());
            assert!(state.snapshots.is_empty());
            assert!(
                !state
                    .calls
                    .iter()
                    .any(|call| call.starts_with("EFFECT inference"))
            );
        }
        fixture.close(Some(instance)).await;
    }
}

#[tokio::test]
async fn configured_initialization_runs_defaults_before_custom_and_stops_on_custom_error() {
    for source in [CODER, REVIEWER] {
        for fail_custom in [false, true] {
            let mut fixture = Fixture::new();
            {
                let mut state = fixture.commands.lock().unwrap();
                state.initialization = Some(
                    json!({"default":[{"command":"CREATE TEMP BLOCK defaults (messages)","arguments":[]},{"command":"INJECT [] TO defaults.messages","arguments":[]}],"custom":[{"command":"CREATE TEMP BLOCK custom (messages)","arguments":[]},{"command":"INJECT [] TO custom.messages","arguments":[]}]}),
                );
                if fail_custom {
                    state.fail = Some("CREATE TEMP BLOCK custom (messages)".into());
                }
            }
            let started = fixture.start(source).await;
            if fail_custom {
                assert!(
                    started
                        .as_ref()
                        .unwrap_err()
                        .to_string()
                        .contains("injected CREATE TEMP BLOCK custom")
                );
            } else {
                assert!(started.is_ok());
            }
            {
                let state = fixture.commands.lock().unwrap();
                let find = |text: &str| state.calls.iter().position(|command| command == text);
                assert!(
                    find("CREATE TEMP BLOCK defaults (messages)").unwrap()
                        < find("INJECT [] TO defaults.messages").unwrap()
                );
                assert!(
                    find("INJECT [] TO defaults.messages").unwrap()
                        < find("CREATE TEMP BLOCK custom (messages)").unwrap()
                );
                if fail_custom {
                    assert!(find("INJECT [] TO custom.messages").is_none());
                    assert!(find("EFFECT ready").is_none());
                } else {
                    assert!(
                        find("CREATE TEMP BLOCK custom (messages)").unwrap()
                            < find("INJECT [] TO custom.messages").unwrap()
                    );
                    assert!(
                        find("INJECT [] TO custom.messages").unwrap()
                            < find("EFFECT ready").unwrap()
                    );
                }
            }
            fixture.close(started.ok()).await;
        }
    }
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}
