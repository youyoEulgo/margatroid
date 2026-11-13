use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};
use lua_runtime_core::*;
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_core::{MclCommand, MclValue, MessageArray};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use workspace_core::{
    WorkspaceExecutionClose, WorkspaceState, spawn_workspace_executor,
};

const CODER: &str = include_str!("../../../../resources/agent-drivers/local/coder/latest/base.lua");
const REVIEWER: &str =
    include_str!("../../../../resources/agent-drivers/local/reviewer/latest/base.lua");

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
            address::Address::new("agent:local/driver-errors:latest").unwrap(),
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
                        MclValue::Config(toml::from_str("sandbox='read_only'\n[compaction]\nthreshold=0.8\nkeep_messages=2\n").unwrap())
                    }
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
                } else if text.starts_with("EFFECT inference ") {
                    MclValue::Inference {message:json!({"type":"assistant", "content":"done", "stop_reason":"completed"}), stop_reason:"completed".into()}
                } else if text == "EFFECT start_turn" {
                    state.working = true;
                    MclValue::Unit
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
        self.app.world().send_event(StartDriverLuaVm {
            owner:self.owner, workspace:self.workspace,
            program:LuaProgram {source:source.into(), origin:"shipped-driver.lua".into(), libraries:LuaStandardLibraries::Minimal},
            environment:LuaEnvironment {globals:BTreeMap::from([("agent_info".into(), LuaBinding::Value(LuaValue::from_json(json!({"entity":{"index":self.owner.index(),"generation":self.owner.generation()}, "model":{"context_window_tokens":4096}})).unwrap()))]), modules:BTreeMap::new()},
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
async fn shipped_drivers_recover_each_command_stage_and_continue_in_same_vm() {
    for source in [CODER, REVIEWER] {
        for stage in [
            "GET entry.pending[0]",
            "INJECT [] TO entry.pending[0,1]",
            "EFFECT history_append ?",
            "INJECT ? TO session.conversation[-1]",
            "EFFECT start_turn",
            "CONFIG GET",
            "EFFECT inference (msg.instructions, session.conversation)",
            "INJECT ? TO entry.pending[-1]",
            "EFFECT finish_turn",
        ] {
            let mut fixture = Fixture::new();
            let instance = fixture.start(source).await.unwrap();
            fixture.settle().await;
            fixture.commands.lock().unwrap().fail = Some(stage.into());
            fixture.wake(&instance);
            let state = fixture.commands.clone();
            fixture
                .drive(|| state.lock().unwrap().fail.is_none().then_some(()))
                .await;
            fixture.settle().await;
            let (before, histories, finishes) = {
                let state = fixture.commands.lock().unwrap();
                assert!(
                    fixture.exits.lock().unwrap().is_empty(),
                    "VM exited at {stage}"
                );
                if stage != "EFFECT history_append ?" {
                    assert!(
                        state
                            .history
                            .iter()
                            .any(|message| message["type"] == "error"),
                        "missing visible error at {stage}"
                    );
                }
                if stage == "EFFECT finish_turn" {
                    assert!(state.working);
                }
                (
                    state.finished,
                    state
                        .calls
                        .iter()
                        .filter(|call| *call == "EFFECT history_append ?")
                        .count(),
                    state
                        .calls
                        .iter()
                        .filter(|call| *call == "EFFECT finish_turn")
                        .count(),
                )
            };
            fixture.settle().await;
            {
                let state = fixture.commands.lock().unwrap();
                assert_eq!(
                    histories,
                    state
                        .calls
                        .iter()
                        .filter(|call| *call == "EFFECT history_append ?")
                        .count()
                );
                assert_eq!(
                    finishes,
                    state
                        .calls
                        .iter()
                        .filter(|call| *call == "EFFECT finish_turn")
                        .count()
                );
            }
            fixture.wake(&instance);
            let state = fixture.commands.clone();
            fixture
                .drive(|| {
                    let state = state.lock().unwrap();
                    (state.finished > before && state.pending.is_empty() && !state.working)
                        .then_some(())
                })
                .await;
            fixture.settle().await;
            assert_eq!(
                fixture
                    .app
                    .world()
                    .get_component::<DriverLuaVm>(fixture.owner)
                    .unwrap()
                    .instance,
                instance
            );
            assert!(fixture.exits.lock().unwrap().is_empty());
            fixture.close(Some(instance)).await;
        }
    }
}

#[tokio::test]
async fn unknown_runtime_errors_become_history_errors_and_next_message_recovers() {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        let instance = fixture.start(source).await.unwrap();
        fixture.settle().await;
        fixture.commands.lock().unwrap().malformed_config = true;
        fixture.wake(&instance);
        let state = fixture.commands.clone();
        fixture
            .drive(|| {
                state
                    .lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|message| message["type"] == "error")
                    .then_some(())
            })
            .await;
        fixture.settle().await;
        {
            let state = fixture.commands.lock().unwrap();
            let error = state
                .history
                .iter()
                .find(|message| message["type"] == "error")
                .unwrap();
            assert_eq!(error["message"], "Driver execution failed; see Agent logs");
        }
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
        let details = logs[0].record.details_json.as_ref().unwrap();
        assert_eq!(details["stage"], "turn");
        assert!(
            details["raw_error"]
                .as_str()
                .unwrap()
                .contains("shipped-driver.lua")
        );
        let before = fixture.commands.lock().unwrap().finished;
        fixture.wake(&instance);
        let state = fixture.commands.clone();
        fixture
            .drive(|| {
                let state = state.lock().unwrap();
                (state.finished > before && state.pending.is_empty() && !state.working)
                    .then_some(())
            })
            .await;
        assert!(fixture.exits.lock().unwrap().is_empty());
        fixture.close(Some(instance)).await;
    }
}

#[tokio::test]
async fn initialization_failure_is_an_error_and_never_signals_ready() {
    for source in [CODER, REVIEWER] {
        let mut fixture = Fixture::new();
        fixture.commands.lock().unwrap().fail = Some("CONFIG GET".into());
        let error = fixture.start(source).await.unwrap_err();
        assert!(error.to_string().contains("injected CONFIG GET"));
        assert!(!error.to_string().contains("stack traceback"));
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
        assert!(
            !fixture
                .commands
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|call| call == "EFFECT ready")
        );
        assert!(
            fixture
                .app
                .world()
                .get_component::<DriverLuaVm>(fixture.owner)
                .is_none()
        );
        fixture.close(None).await;
    }
}

#[tokio::test]
async fn recovery_command_failures_are_bounded_and_cleanup_retries_on_next_wake() {
    for source in [CODER, REVIEWER] {
        for recovery_stage in ["EFFECT history_append ?", "EFFECT finish_turn"] {
            let mut fixture = Fixture::new();
            let instance = fixture.start(source).await.unwrap();
            fixture.settle().await;
            {
                let mut state = fixture.commands.lock().unwrap();
                state.fail = Some("CONFIG GET".into());
                state.recovery_after_failure = Some(recovery_stage.into());
            }
            fixture.wake(&instance);
            let state = fixture.commands.clone();
            fixture
                .drive(|| {
                    let state = state.lock().unwrap();
                    (state.fail.is_none() && state.recovery_fail.is_none()).then_some(())
                })
                .await;
            fixture.settle().await;
            let calls = fixture.commands.lock().unwrap().calls.len();
            let before = fixture.commands.lock().unwrap().finished;
            fixture.settle().await;
            assert_eq!(
                fixture.commands.lock().unwrap().calls.len(),
                calls,
                "recovery busy-looped"
            );
            if recovery_stage == "EFFECT finish_turn" {
                assert!(fixture.commands.lock().unwrap().working);
            }
            fixture.wake(&instance);
            let state = fixture.commands.clone();
            fixture
                .drive(|| {
                    let state = state.lock().unwrap();
                    (state.finished > before && state.pending.is_empty() && !state.working)
                        .then_some(())
                })
                .await;
            assert!(fixture.exits.lock().unwrap().is_empty());
            fixture.close(Some(instance)).await;
        }
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
            let prefix = source
                .split("local initialized, initialization_error")
                .next()
                .unwrap();
            let program = format!(
                r#"{prefix}
local ok = pcall(maybe_compact_context, {{usage = {{input_tokens = {tokens}, output_tokens = 0}}}})
local marker = {{type = "user", content = ok and "probe-ok" or "probe-failed"}}
command("EFFECT history_append ?", {{{{marker}}}})
command("EFFECT ready")
margatroid.driver:wait()
"#
            );
            let instance = fixture.start(&program).await.unwrap();
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
                    assert!(snap < replace);
                }
            }
            fixture.close(Some(instance)).await;
        }
    }
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}
