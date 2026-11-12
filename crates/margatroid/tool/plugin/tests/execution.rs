#![cfg(target_os = "linux")]

#[path = "support/storage.rs"]
mod storage_fixture;

#[path = "support/shell_client.rs"]
mod shell_client;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::{Agent, AgentState};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolConfiguration, ToolError, ToolErrorKind as Kind, ToolExecuteRequest,
    ToolExecuteResponse, ToolExecutionLimits, ToolTasks,
};
use tool_plugin::ToolPlugin;
use tool_resource_core::{read_tool, ToolEntry, Tools};
use workspace_core::{Workspace, WorkspaceState};

fn imported_entry(path: &std::path::Path) -> ToolEntry {
    let tool = read_tool(path).unwrap();
    ToolEntry {
        path: tool.origin,
        description: tool.description,
        input_schema: tool.input_schema,
        owner_task_id: None,
        binding: None,
    }
}

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
    configuration: ToolConfiguration,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(ToolExecutionLimits::default())
    }

    fn with_limits(limits: ToolExecutionLimits) -> Self {
        Self::with_runner(
            limits,
            std::path::Path::new(env!("CARGO_BIN_EXE_tool_runner")),
        )
    }

    fn with_runner(limits: ToolExecutionLimits, runner: &std::path::Path) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(
                ToolPlugin::new()
                    .with_runner_path(runner)
                    .with_limits(limits),
            );
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                definition: None,
                name: "test".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().to_owned(),
                state: WorkspaceState::Running,
                agents: BTreeSet::new(),
            },
        );
        let agent = app.world_mut().spawn();
        storage_fixture::mount(app.world_mut(), agent, root.path());
        app.world_mut().insert_component(
            agent,
            Agent {
                name: "test".into(),
                workspace,
                state: AgentState::Working,
                turn: Some("test-turn".into()),
            },
        );
        let (sender, mut reply) = oneshot::channel();
        app.world().emit_event(InitToolTasks {
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        app.tick();
        assert!(reply.try_recv().unwrap().is_ok());
        Self {
            app,
            agent,
            root,
            configuration: ToolConfiguration { timeout_ms: 30_000 },
        }
    }

    fn with_configuration(configuration: ToolConfiguration) -> Self {
        configuration.validate().unwrap();
        Self {
            configuration,
            ..Self::new()
        }
    }

    fn tool(&mut self, alias: &str, program: &str) {
        // Test programs declare their own lifetimes; bundled programs keep their
        // author-supplied configure function and are never overridden here.
        let program = if program.contains("function configure(") {
            program.to_owned()
        } else {
            format!(
                "function configure(arguments, context) return {{timeout_ms={}}} end\n{program}",
                self.configuration.timeout_ms,
            )
        };
        let directory = self.root.path().join(alias);
        std::fs::create_dir_all(&directory).unwrap();
        if alias == "bash" || alias == "bash-exec" || program.contains("/shell.lua") {
            shell_client::install_shell(&directory);
        }
        std::fs::write(
            directory.join("tool.toml"),
            "schema_version=1\ndescription='test'\n",
        )
        .unwrap();
        std::fs::write(directory.join("input.schema.json"), "{}").unwrap();
        std::fs::write(directory.join("main.lua"), program).unwrap();
        if !self.app.world().contains_component::<Tools>(self.agent) {
            self.app
                .world_mut()
                .insert_component(self.agent, Tools::default());
        }
        self.app
            .world_mut()
            .get_component_mut::<Tools>(self.agent)
            .unwrap()
            .insert(alias.into(), imported_entry(&directory.join("tool.toml")))
            .unwrap();
    }

    fn execute(
        &mut self,
        id: &str,
        alias: &str,
        arguments: Value,
    ) -> oneshot::Receiver<Result<ToolExecuteResponse, ToolError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(ToolExecuteRequest {
            id: id.into(),
            agent: self.agent,
            turn: "test-turn".into(),
            alias: alias.into(),
            call_id: format!("call-{id}"),
            arguments,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        self.app.tick();
        // The next frame dispatches the ordinary launch closure.
        self.app.tick();
        receiver
    }

    fn response(
        &mut self,
        reply: oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
    ) -> Result<ToolExecuteResponse, ToolError> {
        self.response_with_timeout(reply, Duration::from_secs(10))
    }

    fn response_with_timeout(
        &mut self,
        mut reply: oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
        timeout: Duration,
    ) -> Result<ToolExecuteResponse, ToolError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.app.tick();
            match reply.try_recv() {
                Ok(response) => return response,
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(error) => panic!("tool reply closed: {error}"),
            }
            assert!(Instant::now() < deadline, "tool response timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn drive<T: Send + 'static>(
        &mut self,
        future: impl std::future::Future<Output = T> + Send + 'static,
    ) -> T {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let _ = sender.send(future.await);
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.tick();
            match receiver.try_recv() {
                Ok(value) => return value,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("async helper closed: {error}"),
            }
            assert!(Instant::now() < deadline, "async helper timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn tasks(&self) -> ToolTasks {
        self.app
            .world()
            .get_component::<ToolTasks>(self.agent)
            .unwrap()
            .clone()
    }

    fn removed(&mut self, id: &str) {
        let tasks = self.tasks();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.tick();
            match tasks.read(id) {
                Err(error) => {
                    assert_eq!(error.kind, Kind::TaskMissing, "{id}: {error:?}");
                    break;
                }
                Ok(result) => assert!(
                    Instant::now() < deadline,
                    "task {id} still retained: {:?}",
                    result.error
                ),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!tasks.list().iter().any(|(task_id, _)| task_id == id));
        assert_eq!(
            tasks.send(id, Value::Null).unwrap_err().kind,
            Kind::TaskMissing
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            runtime.block_on(tasks.finish(id)).unwrap_err().kind,
            Kind::TaskMissing
        );
    }

    fn output(&mut self, id: &str) -> tool_core::TaskReadResult {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.tick();
            let result = self.tasks().read(id).unwrap();
            if !result.output.is_empty() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "task output timed out: {:?}",
                result.error
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

struct ReleaseFakeSupervisor(std::path::PathBuf);

impl Drop for ReleaseFakeSupervisor {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

#[test]
fn failed_calls_reply_while_uncooperative_supervisor_retains_cleanup() {
    use std::os::unix::fs::PermissionsExt;

    for (mode, expected) in [
        ("timeout", Kind::Timeout),
        ("protocol", Kind::RunnerProtocolFailed),
        ("cancel", Kind::Cancelled),
        ("stopped", Kind::RunnerProtocolFailed),
        ("cleanup", Kind::CleanupFailed),
    ] {
        let scripts = tempfile::tempdir().unwrap();
        let runner = scripts.path().join("fake-supervisor");
        std::fs::write(
            &runner,
            r#"#!/usr/bin/python3
import json, pathlib, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
request = json.loads(sys.stdin.readline())
mode = request['arguments']['mode']
def frame(value):
    print(json.dumps({'protocol_version': 1, 'frame': value}), flush=True)
frame({'type': 'configured', 'configuration': {'timeout_ms': 30 if mode == 'timeout' else 0}})
sys.stdin.readline()
pathlib.Path('ready').touch()
if mode == 'protocol':
    print('invalid protocol', flush=True)
elif mode in ('stopped', 'cleanup'):
    result = {'Ok': None} if mode == 'cleanup' else {'Err': {'kind': 'RunnerProtocolFailed', 'message': 'original stopped error'}}
    frame({'type': 'stopped', 'result': result, 'cleanup_proven': False})
while not pathlib.Path('release').exists():
    time.sleep(0.002)
frame({'type': 'stopped', 'result': {'Ok': None}, 'cleanup_proven': True})
"#,
        )
        .unwrap();
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut fixture = Fixture::with_runner(ToolExecutionLimits::default(), &runner);
        let release = ReleaseFakeSupervisor(fixture.root.path().join("release"));
        fixture.tool("fake", "function execute() end");
        let reply = fixture.execute("pending-cleanup", "fake", json!({"mode": mode}));
        let handle = fixture.tasks().get("pending-cleanup").unwrap();
        if mode == "cancel" {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !fixture.root.path().join("ready").exists() {
                assert!(
                    Instant::now() < deadline,
                    "fake supervisor failed to configure"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            handle.cancel();
        }
        let failure = fixture
            .response_with_timeout(reply, Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(failure.kind, expected, "{mode}: {failure:?}");
        if mode == "stopped" {
            assert_eq!(failure.message, "original stopped error");
        }
        assert!(
            !handle.is_cleanup_complete(),
            "{mode}: unproven cleanup completed"
        );
        let retained = fixture.tasks().read("pending-cleanup").unwrap();
        let retained_error = retained.error.unwrap();
        assert!(
            retained_error.kind == expected || retained_error.kind == Kind::CleanupFailed,
            "{mode}: live task lost failure: {retained_error:?}"
        );
        assert_eq!(fixture.tasks().list().len(), 1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if mode != "cancel" {
            let finishing = handle.clone();
            let finish = fixture.drive(async move {
                tokio::time::timeout(Duration::from_secs(5), finishing.finish()).await
            });
            assert_eq!(finish.unwrap().unwrap_err().kind, Kind::CleanupFailed);
        } else {
            assert_eq!(handle.read().unwrap().error.unwrap().kind, Kind::Cancelled);
        }
        assert!(
            !handle.is_cleanup_complete(),
            "{mode}: finish dropped supervision"
        );
        assert!(fixture.tasks().get("pending-cleanup").is_ok());
        drop(release);
        fixture.removed("pending-cleanup");
        runtime.block_on(handle.finish()).unwrap();
    }
}

#[test]
fn short_return_then_self_finish_stops_even_caught_finish_control() {
    let mut fixture = Fixture::new();
    fixture.tool("short", "function execute(args, context) margatroid.task:return_result('hello'); pcall(function() margatroid.task:finish(margatroid.context.task_id) end); margatroid.process:start('/bin/sh', {'-c','touch unreachable-after-finish'}):wait(); margatroid.task:append_output('unreachable') end");
    let reply = fixture.execute("short-task", "short", json!({}));
    let response = fixture.response(reply).unwrap();
    assert_eq!(response.content, "hello");
    assert_eq!(response.call_id, "call-short-task");
    assert_eq!(response.alias, "short");
    fixture.removed("short-task");
    assert!(!fixture
        .root
        .path()
        .join("unreachable-after-finish")
        .exists());
}

#[test]
fn return_keeps_vm_alive_across_turn_changes_and_supports_json_cache_replacement() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "background",
        r#"function execute(args, context)
        margatroid.task:return_result({task_id=margatroid.context.task_id})
        while true do
            local input = margatroid.task:receive()
            if input.finish then margatroid.task:finish(margatroid.context.task_id) end
            if input.replace then margatroid.task:replace_output(input.value) else margatroid.task:append_output(input) end
        end
    end"#,
    );
    let reply = fixture.execute("background-task", "background", json!({}));
    let response = fixture.response(reply).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response.content).unwrap(),
        json!({"task_id":"background-task"})
    );
    assert!(!fixture
        .tasks()
        .get("background-task")
        .unwrap()
        .is_cleanup_complete());
    let agent = fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.agent)
        .unwrap();
    agent.turn = Some("unrelated-new-turn".into());
    agent.state = AgentState::Idle;
    let value = json!({"nested":[1,true,null,{"text":"hello"}]});
    fixture
        .tasks()
        .send("background-task", value.clone())
        .unwrap();
    let first = fixture.output("background-task");
    assert_eq!(first.output, vec![value]);
    assert!(!first.replaced);

    assert!(fixture
        .tasks()
        .read("background-task")
        .unwrap()
        .output
        .is_empty());
    fixture
        .tasks()
        .send(
            "background-task",
            json!({"replace":true,"value":["replacement",null]}),
        )
        .unwrap();
    let replaced = fixture.output("background-task");
    assert!(replaced.replaced);
    assert_eq!(replaced.output, vec![json!(["replacement", null])]);
    assert!(!fixture.tasks().read("background-task").unwrap().replaced);
    fixture
        .tasks()
        .send("background-task", json!({"finish":true}))
        .unwrap();
    fixture.removed("background-task");
}

#[test]
fn finish_other_task_waits_for_real_cleanup_and_removes_both_entries() {
    let mut fixture = Fixture::new();
    fixture.tool("background", "function execute(args, context) margatroid.task:return_result('started'); margatroid.task:receive(); margatroid.task:append_output('unexpected') end");
    let reply = fixture.execute("target", "background", json!({}));
    fixture.response(reply).unwrap();
    fixture.tool("closer", "function execute(args, context) margatroid.task:finish(margatroid.context.arguments.target); margatroid.task:return_result('closed'); margatroid.task:finish(margatroid.context.task_id) end");
    let reply = fixture.execute("closer", "closer", json!({"target":"target"}));
    assert_eq!(fixture.response(reply).unwrap().content, "closed");
    fixture.removed("target");
    fixture.removed("closer");
    assert!(fixture.tasks().list().is_empty());
}

#[test]
fn lua_can_send_receive_and_read_another_same_agent_task() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "echo",
        r#"function execute(args, context)
        margatroid.task:return_result('ready')
        local input=margatroid.task:receive()
        margatroid.task:append_output(input.value)
        margatroid.task:send(input.reply_to, {ready=true})
        margatroid.task:receive()
    end"#,
    );
    let reply = fixture.execute("echo-task", "echo", json!({}));
    fixture.response(reply).unwrap();
    fixture.tool(
        "reader",
        r#"function execute(args, context)
        margatroid.task:send(margatroid.context.arguments.target, {value={1,true,margatroid.data.null}, reply_to=margatroid.context.task_id})
        margatroid.task:receive()
        local result=margatroid.task:read_output(margatroid.context.arguments.target)
        margatroid.task:finish(margatroid.context.arguments.target)
        margatroid.task:return_result(result)
        margatroid.task:finish(margatroid.context.task_id)
    end"#,
    );
    let reply = fixture.execute("reader-task", "reader", json!({"target":"echo-task"}));
    let response = fixture.response(reply).unwrap();
    let result: Value = serde_json::from_str(&response.content).unwrap();
    assert_eq!(result["output"], json!([[1, true, null]]));
    assert!(result.get("completed").is_none());
    fixture.removed("reader-task");
    fixture.removed("echo-task");
}

#[test]
fn natural_exit_without_return_reports_missing_return_and_late_errors_are_removed() {
    let mut fixture = Fixture::new();
    fixture.tool("missing", "function execute() return 'not task_return' end");
    let reply = fixture.execute("missing", "missing", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::MissingReturn
    );
    fixture.removed("missing");
    fixture.tool(
        "late",
        "function execute() margatroid.task:return_result('once'); local ok, err=pcall(function() margatroid.task:return_result('twice') end); margatroid.task:append_output({ok=ok,error=tostring(err)}); margatroid.task:receive() end",
    );
    let reply = fixture.execute("late", "late", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "once");
    let result = fixture.output("late");
    assert_eq!(result.output[0]["ok"], false);
    assert!(result.output[0]["error"]
        .as_str()
        .unwrap()
        .contains("already"));
    finish_task(&mut fixture, "late");
}

#[test]
fn recorded_schema_is_authoritative_while_program_is_read_for_each_execution() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "fresh",
        "function execute(args,context) margatroid.task:return_result('first'); margatroid.task:finish(margatroid.context.task_id) end",
    );
    let reply = fixture.execute("first", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "first");
    fixture.removed("first");
    std::fs::write(
        fixture.root.path().join("fresh/main.lua"),
        "function configure() return {timeout_ms=30000} end function execute(args,context) margatroid.task:return_result('changed'); margatroid.task:finish(margatroid.context.task_id) end",
    ).unwrap();
    let reply = fixture.execute("second", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "changed");
    fixture.removed("second");
    std::fs::write(
        fixture.root.path().join("fresh/input.schema.json"),
        r#"{"type":"object","required":["required"]}"#,
    )
    .unwrap();
    let reply = fixture.execute("recorded-schema", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "changed");
    fixture.removed("recorded-schema");
    let path = fixture.root.path().join("fresh/main.lua");
    assert_eq!(
        path.parent(),
        Some(fixture.root.path().join("fresh").as_path())
    );
    std::fs::remove_file(path).unwrap();
    let reply = fixture.execute("missing-file", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::ReadFailed);
    fixture.removed("missing-file");
}

#[test]
fn independent_supervisor_reaps_setsid_descendants_after_return_and_finish() {
    let mut fixture = Fixture::new();
    fixture.tool("escape", r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
        client.exec("setsid /bin/sh -c 'echo $$ > escaped.pid; exec sleep 15' </dev/null >/dev/null 2>&1 &")
        client.exec("while [ ! -s escaped.pid ]; do sleep 0.01; done")
        margatroid.task:return_result('spawned')
        margatroid.task:finish(margatroid.context.task_id)
    end"#);
    let reply = fixture.execute("escape", "escape", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "spawned");
    fixture.removed("escape");
    let pid: i32 = std::fs::read_to_string(fixture.root.path().join("escaped.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}

#[test]
fn init_preserves_existing_tasks_and_closed_agent_rejects_new_calls() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "background",
        "function execute() margatroid.task:return_result('started'); margatroid.task:receive() end",
    );
    let reply = fixture.execute("background", "background", json!({}));
    fixture.response(reply).unwrap();
    let (sender, mut reply) = oneshot::channel();
    fixture.app.world().emit_event(InitToolTasks {
        agent: fixture.agent,
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    fixture.app.tick();
    assert!(reply.try_recv().unwrap().is_ok());
    assert_eq!(fixture.tasks().list().len(), 1);
    fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.agent)
        .unwrap()
        .state = AgentState::Stopping;
    let reply = fixture.execute("new", "background", json!({}));
    let rejected = fixture.response(reply).unwrap();
    assert!(rejected.failed);
    assert_eq!(rejected.call_id, "call-new");
    assert_eq!(rejected.alias, "background");
    assert_eq!(rejected.content, "Tool call aborted.");
    assert_eq!(
        fixture.tasks().read("new").unwrap_err().kind,
        Kind::TaskMissing
    );
    assert_eq!(fixture.tasks().list().len(), 1);
    assert!(!fixture
        .tasks()
        .get("background")
        .unwrap()
        .is_cleanup_complete());
    fixture.tasks().cancel("background").unwrap();
    fixture.removed("background");
}

#[test]
fn reply_receiver_drop_does_not_cancel_a_registered_task() {
    let mut fixture = Fixture::new();
    fixture.tool("detached", "function execute(args,context) margatroid.task:return_result('ignored'); margatroid.task:append_output('alive'); margatroid.task:receive(); margatroid.task:finish(margatroid.context.task_id) end");
    let reply = fixture.execute("detached", "detached", json!({}));
    drop(reply);
    assert_eq!(fixture.output("detached").output, vec![json!("alive")]);
    assert!(!fixture
        .tasks()
        .get("detached")
        .unwrap()
        .is_cleanup_complete());
    fixture.tasks().send("detached", json!(null)).unwrap();
    fixture.removed("detached");
}

#[test]
fn small_task_cache_rolls_over_without_rejecting_the_result() {
    let mut fixture = Fixture::with_limits(ToolExecutionLimits {
        cache_bytes: 8,
        ..Default::default()
    });
    fixture.tool(
        "cache",
        r#"function execute()
        margatroid.task:append_output('old')
        margatroid.task:append_output('discard你好世界')
        margatroid.task:return_result('text-larger-than-cache')
        margatroid.task:receive()
    end"#,
    );
    let reply = fixture.execute("cache", "cache", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap().content,
        "text-larger-than-cache"
    );
    let read = fixture.tasks().read("cache").unwrap();
    assert!(read.error.is_none());
    let text = read.output[0].as_str().unwrap();
    assert!("discard你好世界".ends_with(text));
    assert!(text.ends_with('界'));
    assert!(!text.contains("old"));
    fixture.tasks().cancel("cache").unwrap();
    fixture.removed("cache");
}

#[test]
fn lua_vm_allocation_still_obeys_the_memory_budget() {
    let mut fixture = Fixture::with_limits(ToolExecutionLimits {
        memory_bytes: 2 * 1024 * 1024,
        ..Default::default()
    });
    fixture.tool("memory", "function execute() local values={} for i=1,10000 do values[i]=string.rep(tostring(i),4096) end margatroid.task:return_result('unexpected') end");
    let reply = fixture.execute("memory", "memory", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::ExecutionFailed
    );
    fixture.removed("memory");
}

#[test]
fn infinite_execution_stops_at_the_explicit_task_timeout() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 100 });
    fixture.tool("loop", "function execute() while true do end end");
    let reply = fixture.execute("loop", "loop", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::Timeout);
    fixture.removed("loop");
}

#[test]
fn unlimited_infinite_execution_is_explicitly_cancellable() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 0 });
    fixture.tool(
        "loop",
        "function execute() margatroid.task:append_output('running'); while true do end end",
    );
    let reply = fixture.execute("loop", "loop", json!({}));
    assert_eq!(fixture.output("loop").output, [json!("running")]);
    fixture.tasks().cancel("loop").unwrap();
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::Cancelled);
    fixture.removed("loop");
}

#[test]
fn result_images_are_validated_by_the_core_across_the_runner_protocol() {
    let mut fixture = Fixture::new();
    fixture.tool("image", "function execute(args, context) margatroid.task:return_result({ok=true,content='image',images={{media_type='image/png',data='iVBORw0KGgo='}}}); margatroid.task:finish(margatroid.context.task_id) end");
    let reply = fixture.execute("image", "image", json!({}));
    let result = fixture.response(reply).unwrap();
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].media_type, "image/png");
    fixture.removed("image");
    fixture.tool("invalid-image", "function execute() margatroid.task:return_result({ok=true,content='image',images={{media_type='image/png',data='eA=='}}}) end");
    let reply = fixture.execute("invalid-image", "invalid-image", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::RunnerProtocolFailed
    );
    fixture.removed("invalid-image");
}

#[test]
fn bundled_bash_exec_sleep_eleven_seconds_succeeds_with_author_configured_budget() {
    let limits = ToolExecutionLimits::default();
    let mut fixture = Fixture::with_limits(limits);
    fixture.tool(
        "bash-exec",
        include_str!("../../../../../resources/tools/local/bash-exec/latest/main.lua"),
    );
    let reply = fixture.execute(
        "sleep-eleven",
        "bash-exec",
        json!({"cmd": "sleep 11; printf sleep-eleven-completed"}),
    );
    let response = fixture
        .response_with_timeout(reply, Duration::from_secs(20))
        .unwrap();
    assert!(!response.failed, "{response:?}");
    assert_eq!(response.content, "sleep-eleven-completed\n[exit code: 0]");
    fixture.removed("sleep-eleven");
}

#[test]
fn shell_obeys_only_the_task_lifetime_including_unlimited() {
    for timeout_ms in [0, 3_000] {
        let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms });
        fixture.tool("slow-shell", "function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context); local result=client.exec('sleep 0.25; printf done'); margatroid.task:return_result(result.stdout); margatroid.task:finish(margatroid.context.task_id) end");
        let reply = fixture.execute("slow-shell", "slow-shell", json!({}));
        assert_eq!(fixture.response(reply).unwrap().content, "done");
        fixture.removed("slow-shell");
    }
}

#[test]
fn task_lifetime_reports_timeout_after_real_descendant_cleanup() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 250 });
    fixture.tool(
        "shell-timeout",
        r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
        client.exec("echo $$ > shell.pid; sleep 15 & echo $! > child.pid; wait")
        margatroid.task:return_result('unexpected success')
    end"#,
    );
    let reply = fixture.execute("shell-timeout", "shell-timeout", json!({}));
    let error = fixture.response(reply).unwrap_err();
    assert_eq!(error.kind, Kind::Timeout);
    fixture.removed("shell-timeout");

    for name in ["shell.pid", "child.pid"] {
        let pid = std::fs::read_to_string(fixture.root.path().join(name))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH),
            "{name} survived the timeout response"
        );
    }
}

#[test]
fn total_execution_deadline_still_bounds_shell_work() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 500 });
    fixture.tool(
        "total-timeout",
        r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
        client.exec("echo $$ > total-shell.pid; exec sleep 15")
        margatroid.task:return_result('unexpected success')
    end"#,
    );
    let reply = fixture.execute("total-timeout", "total-timeout", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::Timeout);
    fixture.removed("total-timeout");

    let pid = std::fs::read_to_string(fixture.root.path().join("total-shell.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}

#[test]
fn tool_local_shell_timeouts_are_rejected_before_spawning_a_child() {
    let mut fixture = Fixture::new();
    for (index, timeout) in ["0", "250", "-1", "0.5", "math.huge", "'250'", "true"]
        .iter()
        .enumerate()
    {
        let id = format!("invalid-timeout-{index}");
        fixture.tool(
            &id,
            &format!(
                "function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context); client.exec('touch unexpected-child', {{timeout_ms={timeout}}}) end"
            ),
        );
        let reply = fixture.execute(&id, &id, json!({}));
        let error = fixture.response(reply).unwrap_err();
        assert_eq!(error.kind, Kind::ExecutionFailed, "{timeout}: {error:?}");
        assert!(
            error.message.contains("timeout_ms belongs in configure()"),
            "{error:?}"
        );
        fixture.removed(&id);
        assert!(!fixture.root.path().join("unexpected-child").exists());
    }
}

#[test]
fn execution_timeout_still_reaps_a_background_vm_after_return() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 200 });
    fixture.tool(
        "timeout",
        "function execute() margatroid.task:return_result('started'); margatroid.task:receive() end",
    );
    let reply = fixture.execute("timeout", "timeout", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "started");
    fixture.removed("timeout");
}

#[test]
fn selected_sandbox_is_read_fresh_and_invalid_policy_never_starts_tool() {
    let mut fixture = Fixture::new();
    fixture.tool("safe", "function execute(args,context) margatroid.task:append_output('must-not-run'); margatroid.task:return_result('unsafe'); margatroid.task:finish(margatroid.context.task_id) end");
    let policy = fixture.root.path().join("policy.json");
    std::fs::write(&policy, "[]").unwrap();
    let mut sandboxes = sandbox_resource_core::Sandboxes::default();
    sandboxes.insert("selected".into(), policy);
    fixture
        .app
        .world_mut()
        .insert_component(fixture.agent, sandboxes);
    fixture.app.world_mut().insert_component(
        fixture.agent,
        sandbox_resource_core::SelectedSandbox {
            alias: Some("selected".into()),
        },
    );
    let reply = fixture.execute("safe", "safe", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::ReadFailed);
    fixture.removed("safe");
}

#[test]
fn duplicate_task_id_does_not_replace_the_existing_shared_handle() {
    let mut fixture = Fixture::new();
    fixture.tool("background", "function execute(args,context) margatroid.task:return_result('first'); margatroid.task:receive(); margatroid.task:finish(margatroid.context.task_id) end");
    let reply = fixture.execute("same", "background", json!({}));
    fixture.response(reply).unwrap();
    let reply = fixture.execute("same", "background", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::DuplicateTask
    );
    assert_eq!(fixture.tasks().list().len(), 1);
    fixture.tasks().send("same", json!({})).unwrap();
    fixture.removed("same");
}

#[test]
fn bundled_bash_stays_alive_after_reply_and_preserves_shell_state() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "bash",
        include_str!("../../../../../resources/tools/local/bash/latest/main.lua"),
    );
    let reply = fixture.execute("terminal", "bash", json!({}));
    let response = fixture.response(reply).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response.content).unwrap()["task_id"],
        "terminal"
    );
    assert!(!fixture
        .tasks()
        .get("terminal")
        .unwrap()
        .is_cleanup_complete());
    fixture.tasks().send("terminal", json!({"text":"export MARGATROID_TEST_VALUE=persistent; mkdir -p child; cd child", "enter":true})).unwrap();
    fixture.tasks().send("terminal", json!({"text":"printf '\\nRESULT=%s DIRECTORY=%s\\n' \"$MARGATROID_TEST_VALUE\" \"${PWD##*/}\"", "enter":true})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while !output.contains("RESULT=persistent DIRECTORY=child") {
        let result = fixture.tasks().read("terminal").unwrap();
        assert!(result.error.is_none(), "{:?}", result.error);
        for value in result.output {
            if let Some(text) = value.as_str() {
                output.push_str(text);
            }
        }
        assert!(Instant::now() < deadline, "terminal output: {output}");
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture
        .tasks()
        .send("terminal", json!({"text":"exit 0", "enter":true}))
        .unwrap();
    fixture.removed("terminal");
}

#[test]
fn terminal_slow_reader_receives_all_eighty_scrolled_lines_once() {
    for repeated in [false, true] {
        let mut fixture = Fixture::new();
        fixture.tool(
            "bash",
            include_str!("../../../../../resources/tools/local/bash/latest/main.lua"),
        );
        let reply = fixture.execute("history", "bash", json!({}));
        fixture.response(reply).unwrap();
        let format = if repeated { "[same]" } else { "[%03d]" };
        fixture.tasks().send("history", json!({"text":format!("for i in {{1..80}}; do printf '{format}\\n' $i; sleep 0.002; done; touch finished"),"enter":true})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fixture.root.path().join("finished").exists() {
            fixture.app.tick();
            assert!(Instant::now() < deadline, "terminal did not finish output");
            std::thread::sleep(Duration::from_millis(2));
        }
        // The terminal reader has been running throughout, but the consumer
        // deliberately waits until every line has scrolled through the PTY.
        let result = fixture.tasks().read("history").unwrap();
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            !result.replaced,
            "ordinary terminal scrolling replaced the cache"
        );
        let output = result
            .output
            .iter()
            .filter_map(Value::as_str)
            .collect::<String>();
        let lines = output
            .lines()
            .filter(|line| line.starts_with('['))
            .collect::<Vec<_>>();
        let expected = (1..=80)
            .map(|index| {
                if repeated {
                    "[same]".to_string()
                } else {
                    format!("[{index:03}]")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, expected, "terminal output: {output}");
        let next = fixture.tasks().read("history").unwrap();
        assert!(!next
            .output
            .iter()
            .filter_map(Value::as_str)
            .any(|text| text.contains("[001]") || text.contains("[same]")));
        fixture
            .tasks()
            .send("history", json!({"text":"exit 0","enter":true}))
            .unwrap();
        fixture.removed("history");
    }
}

#[test]
fn terminal_alternate_screen_updates_replace_cache_without_escape_sequences() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "bash",
        include_str!("../../../../../resources/tools/local/bash/latest/main.lua"),
    );
    let reply = fixture.execute("screen", "bash", json!({}));
    fixture.response(reply).unwrap();
    fixture.tasks().send("screen", json!({"text":"printf '\\033[?1049h\\033[2J\\033[HSCREEN_ONE'; sleep 0.05; printf '\\033[HSCREEN_TWO\\033[K'", "enter":true})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut replaced = false;
    loop {
        let result = fixture.tasks().read("screen").unwrap();
        replaced |= result.replaced;
        let text = result
            .output
            .iter()
            .filter_map(Value::as_str)
            .collect::<String>();
        assert!(!text.contains('\u{1b}'));
        if replaced && text.contains("SCREEN_TWO") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no rendered replacement screen: {text}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture
        .tasks()
        .send(
            "screen",
            json!({"text":"printf '\\033[?1049l'; exit 0", "enter":true}),
        )
        .unwrap();
    fixture.removed("screen");
}

#[test]
fn pending_task_mailbox_preserves_all_values_without_blocking_the_original_reply() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "mailbox",
        r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
        client.exec("sleep 0.1")
        margatroid.task:return_result({ ready = true })
        while true do
            local value = margatroid.task:receive()
            if value == "stop" then break end
            margatroid.task:append_output(value)
        end
        margatroid.task:finish(margatroid.context.task_id)
    end"#,
    );
    let reply = fixture.execute("mailbox", "mailbox", json!({}));
    let accepted = 256;
    for index in 0..accepted {
        fixture.tasks().send("mailbox", json!(index)).unwrap();
    }
    assert_eq!(fixture.response(reply).unwrap().content, "{\"ready\":true}");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = 0;
    while received < accepted {
        received += fixture.tasks().read("mailbox").unwrap().output.len();
        assert!(Instant::now() < deadline, "mailbox lost accepted output");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(received, accepted);
    fixture.tasks().send("mailbox", json!("stop")).unwrap();
    fixture.removed("mailbox");
}

#[test]
fn bash_exit_finishes_task_even_if_a_background_process_keeps_the_pty_open() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "bash",
        include_str!("../../../../../resources/tools/local/bash/latest/main.lua"),
    );
    let reply = fixture.execute("background-child", "bash", json!({}));
    fixture.response(reply).unwrap();
    fixture
        .tasks()
        .send(
            "background-child",
            json!({"text":"sleep 15 & disown; exit 0", "enter":true}),
        )
        .unwrap();
    let started = Instant::now();
    fixture.removed("background-child");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn unlimited_task_waits_for_input_and_other_task_finish_proves_cleanup() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 0 });
    fixture.tool("waiting", "function execute(arguments, context) margatroid.task:return_result(margatroid.context.task_id); local value=margatroid.task:receive(); margatroid.task:append_output(value); margatroid.task:receive() end");
    let reply = fixture.execute("waiting", "waiting", json!({}));
    fixture.response(reply).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!fixture
        .tasks()
        .get("waiting")
        .unwrap()
        .is_cleanup_complete());
    fixture
        .tasks()
        .send("waiting", json!({"continued":true}))
        .unwrap();
    assert_eq!(
        fixture.output("waiting").output,
        vec![json!({"continued":true})]
    );
    fixture.tool(
        "closer-unlimited",
        "function execute() margatroid.task:finish('waiting'); margatroid.task:return_result('closed') end",
    );
    let reply = fixture.execute("closer-unlimited", "closer-unlimited", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "closed");
    fixture.removed("waiting");
}

fn with_daemon_controlling_terminal(name: &str, test: impl FnOnce()) {
    const CHILD: &str = "MARGATROID_CONTROLLING_TERMINAL_TEST";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let tty = std::fs::File::open("/dev/tty").expect("daemon must have a controlling terminal");
        use std::os::fd::AsRawFd;
        assert_eq!(
            unsafe { nix::libc::tcgetpgrp(tty.as_raw_fd()) },
            nix::unistd::getpgrp().as_raw()
        );
        assert_eq!(nix::unistd::getsid(None).unwrap(), nix::unistd::getpid());
        test();
        return;
    }
    shell_client::shell_client();
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(r#"
import ctypes, fcntl, os, pty, signal, subprocess, sys, tempfile, termios, time
assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
master, slave = pty.openpty()

def controlling_terminal():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    os.tcsetpgrp(slave, os.getpgrp())
    signal.signal(signal.SIGTTOU, signal.SIG_DFL)
    signal.signal(signal.SIGTTIN, signal.SIG_DFL)

def descendants():
    processes = {}
    for entry in os.scandir('/proc'):
        if not entry.name.isdigit():
            continue
        try:
            with open(entry.path + '/stat') as source:
                fields = source.read().rsplit(')', 1)[1].split()
            processes[int(entry.name)] = (int(fields[1]), fields[0], fields[2], fields[3])
        except (FileNotFoundError, ProcessLookupError):
            pass
    found = {os.getpid()}
    while True:
        added = {pid for pid, value in processes.items() if value[0] in found} - found
        if not added:
            break
        found.update(added)
    return {pid: processes[pid] for pid in found if pid != os.getpid()}

status = 1
with tempfile.TemporaryFile() as log:
    child = subprocess.Popen([sys.argv[1], '--exact', sys.argv[2], '--nocapture', '--test-threads=1'],
        stdin=slave, stdout=log, stderr=log, preexec_fn=controlling_terminal)
    os.close(slave)
    try:
        deadline = time.monotonic() + 15
        observed_stops = set()
        while child.poll() is None:
            stopped = {pid: value for pid, value in descendants().items() if value[1] in ('T', 't')}
            if stopped.keys() - observed_stops:
                print('observed stopped test processes (pid: ppid,state,pgrp,session):', stopped, file=sys.stderr)
                observed_stops.update(stopped)
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(child.args, 15)
            time.sleep(0.01)
        status = child.returncode
    except subprocess.TimeoutExpired:
        print('controlling-terminal regression timed out; descendants:', descendants(), file=sys.stderr)
    finally:
        remaining = descendants()
        if remaining:
            print('test-owned process cleanup (pid: ppid,state,pgrp,session):', remaining, file=sys.stderr)
        for pid in remaining:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        child.wait()
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                break
            if pid == 0:
                time.sleep(0.01)
        os.close(master)
    log.seek(0)
    sys.stdout.buffer.write(log.read())
sys.exit(status if status >= 0 else 1)
"#)
        .arg(std::env::current_exe().unwrap())
        .arg(name)
        .env(CHILD, name)
        .output()
        .expect("python3 is required for the controlling-terminal regression");
    assert!(
        output.status.success(),
        "PTY child failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runner_does_not_inherit_daemon_controlling_terminal() {
    with_daemon_controlling_terminal(
        "runner_does_not_inherit_daemon_controlling_terminal",
        || {
            let mut fixture = Fixture::new();
            fixture.tool(
                "raw-mode",
                r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
                local result = client.exec([=[python3 - <<'PY'
import errno, os, signal, termios
signal.signal(signal.SIGTTOU, signal.SIG_DFL)
try:
    fd = os.open('/dev/tty', os.O_RDWR)
except OSError as error:
    assert error.errno == errno.ENXIO, error
    print('NO_CONTROLLING_TERMINAL:ENXIO')
else:
    attrs = termios.tcgetattr(fd)
    attrs[3] &= ~(termios.ICANON | termios.ECHO)
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
    raise AssertionError('runner inherited daemon controlling terminal')
PY]=])
                margatroid.task:return_result(result)
                margatroid.task:finish(margatroid.context.task_id)
            end"#,
            );
            let reply = fixture.execute("raw-mode", "raw-mode", json!({}));
            let response = fixture.response(reply).unwrap();
            let result: Value = serde_json::from_str(&response.content).unwrap();
            assert_eq!(result["exit_code"], 0, "{result}");
            assert_eq!(result["stdout"], "NO_CONTROLLING_TERMINAL:ENXIO\n");
            fixture.removed("raw-mode");
        },
    );
}

#[test]
fn cancelling_stopped_runner_group_under_controlling_terminal_reaps_children() {
    with_daemon_controlling_terminal(
        "cancelling_stopped_runner_group_under_controlling_terminal_reaps_children",
        || {
            use nix::sys::signal::{kill, killpg, Signal};
            use nix::unistd::Pid;

            let mut fixture = Fixture::new();
            fixture.tool(
                "stopped",
                r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
                local result = client.exec([=[python3 - <<'PY'
import json, os
worker = os.getpid()
while True:
    with open('/proc/%d/cmdline' % worker, 'rb') as source:
        command = source.read().split(b'\0')
    if b'--margatroid-tool-worker' in command:
        break
    with open('/proc/%d/stat' % worker) as source:
        worker = int(source.read().rsplit(')', 1)[1].split()[1])
    assert worker > 1, 'supervised worker missing from process ancestry'
print(json.dumps(dict(group=os.getpgid(worker), worker=worker)))
PY]=])
                margatroid.task:return_result(result.stdout)
                margatroid.task:receive()
                margatroid.task:finish(margatroid.context.task_id)
            end"#,
            );
            let reply = fixture.execute("stopped", "stopped", json!({}));
            let response = fixture.response(reply).unwrap();
            let pids: Value = serde_json::from_str(&response.content).unwrap();
            let group = Pid::from_raw(pids["group"].as_i64().unwrap() as i32);
            let worker = Pid::from_raw(pids["worker"].as_i64().unwrap() as i32);
            assert_ne!(group, nix::unistd::getpgrp());
            assert_eq!(nix::unistd::getsid(Some(group)).unwrap(), group);
            killpg(group, Signal::SIGSTOP).unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{group}/stat")).unwrap();
                if stat.rsplit_once(") ").unwrap().1.starts_with("T ") {
                    break;
                }
                assert!(Instant::now() < deadline, "supervisor did not stop: {stat}");
                std::thread::sleep(Duration::from_millis(2));
            }
            let started = Instant::now();
            fixture.tasks().cancel("stopped").unwrap();
            fixture.removed("stopped");
            assert!(started.elapsed() < Duration::from_secs(3));

            assert_eq!(kill(group, None), Err(nix::errno::Errno::ESRCH));
            assert_eq!(kill(worker, None), Err(nix::errno::Errno::ESRCH));
            assert_eq!(killpg(group, None), Err(nix::errno::Errno::ESRCH));
        },
    );
}

#[test]
fn interactive_bash_gets_its_own_controlling_terminal_under_tty_daemon() {
    with_daemon_controlling_terminal(
        "interactive_bash_gets_its_own_controlling_terminal_under_tty_daemon",
        || {
            let mut fixture = Fixture::new();
            fixture.tool(
                "bash",
                include_str!("../../../../../resources/tools/local/bash/latest/main.lua"),
            );
            let reply = fixture.execute("own-pty", "bash", json!({}));
            fixture.response(reply).unwrap();
            fixture.tasks().send("own-pty", json!({"text": "python3 -c 'import os,termios; fd=os.open(\"/dev/tty\",os.O_RDWR); assert os.isatty(0); assert os.tcgetpgrp(fd)==os.getpgrp(); attrs=termios.tcgetattr(fd); termios.tcsetattr(fd,termios.TCSANOW,attrs); print(\"OWN_\"+\"PTY_OK\")'", "enter": true})).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut output = String::new();
            while !output.contains("OWN_PTY_OK") {
                let result = fixture.tasks().read("own-pty").unwrap();
                assert!(result.error.is_none(), "{:?}", result.error);
                for value in result.output {
                    if let Some(text) = value.as_str() {
                        output.push_str(text);
                    }
                }
                assert!(Instant::now() < deadline, "terminal output: {output}");
                std::thread::sleep(Duration::from_millis(2));
            }
            fixture
                .tasks()
                .send("own-pty", json!({"text": "exit 0", "enter": true}))
                .unwrap();
            fixture.removed("own-pty");
        },
    );
}

fn finish_task(fixture: &mut Fixture, id: &str) {
    let tasks = fixture.tasks();
    let task_id = id.to_owned();
    fixture
        .drive(async move { tasks.finish(&task_id).await })
        .unwrap();
    fixture.removed(id);
}

#[test]
fn accepted_batch_abort_returns_long_call_immediately_and_preserves_returned_background() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 0 });
    fixture.tool(
        "echo-abort",
        r#"function execute(args, context)
        margatroid.task:return_result('background-ready')
        while true do margatroid.task:append_output(margatroid.task:receive()) end
    end"#,
    );
    fixture.tool(
        "long-abort",
        r#"function execute(args, context) local client=dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context);
        client.exec("sleep 60 & echo $! > abort-child.pid; wait")
        margatroid.task:return_result('unexpected natural result')
    end"#,
    );
    // Emit the entire batch before any system gets to accept either request.
    let mut replies = Vec::new();
    for (id, alias) in [
        ("returned-abort", "echo-abort"),
        ("unreturned-abort", "long-abort"),
    ] {
        let (sender, receiver) = oneshot::channel();
        fixture.app.world().emit_event(ToolExecuteRequest {
            id: id.into(),
            agent: fixture.agent,
            turn: "test-turn".into(),
            alias: alias.into(),
            call_id: format!("original-{id}"),
            arguments: json!({}),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        replies.push(receiver);
    }
    fixture.app.tick();
    let long_reply = replies.pop().unwrap();
    assert_eq!(
        fixture.response(replies.pop().unwrap()).unwrap().content,
        "background-ready"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.root.path().join("abort-child.pid").exists() {
        fixture.app.tick();
        assert!(Instant::now() < deadline, "long shell never started");
        std::thread::sleep(Duration::from_millis(2));
    }
    let pid: i32 = std::fs::read_to_string(fixture.root.path().join("abort-child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(fixture.tasks().list().len(), 2);
    fixture
        .app
        .world_mut()
        .get_component_mut::<Agent>(fixture.agent)
        .unwrap()
        .turn = None;
    let started = Instant::now();
    assert_eq!(fixture.tasks().abort_unreturned(), 1);
    let response = fixture
        .response_with_timeout(long_reply, Duration::from_secs(1))
        .unwrap();
    assert!(started.elapsed() <= Duration::from_secs(1));
    assert!(response.failed);
    assert_eq!(response.call_id, "original-unreturned-abort");
    assert_eq!(response.alias, "long-abort");
    assert_eq!(response.content, "Tool call aborted.");
    assert_eq!(fixture.tasks().abort_unreturned(), 0);
    fixture.removed("unreturned-abort");
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
    fixture
        .tasks()
        .send("returned-abort", json!({"echo":"after abort"}))
        .unwrap();
    assert_eq!(
        fixture.output("returned-abort").output,
        vec![json!({"echo":"after abort"})]
    );
    finish_task(&mut fixture, "returned-abort");
    assert!(fixture.tasks().list().is_empty());
}

const DYNAMIC_PROGRAM: &str = r#"
function configure(args, context)
    if margatroid.context.owner_task_id ~= nil then
        assert(margatroid.context.owner_task_id == 'owner')
        assert(margatroid.context.binding.operation == margatroid.context.alias)
    end
    return {timeout_ms=0}
end
function execute(args, context)
    if margatroid.context.owner_task_id == nil then
        margatroid.tools:register({
            {owner_task_id=margatroid.context.task_id, alias='dynamic_text', description='text operation', input_schema={type='object',required={'text'},properties={text={type='string'}}},binding={operation='dynamic_text'}},
            {owner_task_id=margatroid.context.task_id, alias='dynamic_number', description='number operation', input_schema={type='object',required={'number'},properties={number={type='integer'}}},binding={operation='dynamic_number'}}
        })
        margatroid.task:return_result(margatroid.context.task_id)
        local pending={}
        while true do
            local request=margatroid.task:receive()
            margatroid.task:append_output({request_id=request.request_id,alias=request.alias})
            if not request.hold then
                pending[#pending+1]=request
                if #pending == 2 then
                    for i=2,1,-1 do
                        local item=pending[i]
                        margatroid.task:send(item.reply_to,{request_id=item.request_id,value=item.value,alias=item.alias,resource_dir=margatroid.context.resource_dir})
                        margatroid.task:append_output({responded=item.request_id})
                    end
                    pending={}
                end
            end
        end
    else
        if margatroid.context.arguments.returned then
            margatroid.task:return_result({alive=margatroid.context.task_id})
            while true do margatroid.task:append_output(margatroid.task:receive()) end
        end
        margatroid.task:send(margatroid.context.owner_task_id,{reply_to=margatroid.context.task_id,request_id=margatroid.context.call_id,value=margatroid.context.arguments.text or margatroid.context.arguments.number,alias=margatroid.context.alias,hold=margatroid.context.arguments.hold})
        local response=margatroid.task:receive()
        assert(response.request_id == margatroid.context.call_id)
        assert(response.resource_dir == margatroid.context.resource_dir)
        margatroid.task:return_result(response)
        margatroid.task:finish(margatroid.context.task_id)
    end
end
"#;

#[test]
fn dynamic_aliases_use_distinct_recorded_schemas_and_reverse_concurrent_responses() {
    let mut f = Fixture::new();
    f.tool("factory", DYNAMIC_PROGRAM);
    let reply = f.execute("owner", "factory", json!({}));
    assert_eq!(f.response(reply).unwrap().content, "owner");
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    let text = tools.get("dynamic_text").unwrap();
    let number = tools.get("dynamic_number").unwrap();
    assert_eq!(text.path, number.path);
    assert_ne!(text.input_schema, number.input_schema);
    assert_eq!(text.owner_task_id.as_deref(), Some("owner"));
    assert_eq!(text.binding, Some(json!({"operation":"dynamic_text"})));
    let bad = f.execute("wrong-schema", "dynamic_number", json!({"text":"wrong"}));
    assert_eq!(f.response(bad).unwrap_err().kind, Kind::InvalidArguments);
    f.removed("wrong-schema");
    let first = f.execute(
        "text-call",
        "dynamic_text",
        json!({"text":"hello","owner_task_id":"spoof","binding":{"operation":"spoof"}}),
    );
    let forwarded = f.output("owner");
    assert_eq!(forwarded.output[0]["request_id"], "call-text-call");
    let second = f.execute("number-call", "dynamic_number", json!({"number":42}));
    let second = serde_json::from_str::<Value>(&f.response(second).unwrap().content).unwrap();
    let first = serde_json::from_str::<Value>(&f.response(first).unwrap().content).unwrap();
    assert_eq!(first["request_id"], "call-text-call");
    assert_eq!(first["value"], "hello");
    assert_eq!(second["request_id"], "call-number-call");
    assert_eq!(second["value"], 42);
    assert_eq!(first["resource_dir"], json!(f.root.path().join("factory")));
    f.removed("text-call");
    f.removed("number-call");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut responses = Vec::new();
    while responses.len() < 2 {
        f.app.tick();
        responses.extend(
            f.tasks()
                .read("owner")
                .unwrap()
                .output
                .into_iter()
                .filter_map(|value| value.get("responded").cloned()),
        );
        assert!(Instant::now() < deadline, "owner response log timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        responses,
        vec![json!("call-number-call"), json!("call-text-call")]
    );
    finish_task(&mut f, "owner");
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert!(tools.get("factory").is_some());
    assert!(tools.get("dynamic_text").is_none());
    assert!(tools.get("dynamic_number").is_none());
}

#[test]
fn owner_finish_removes_aliases_without_cancelling_pending_or_returned_tasks() {
    let mut f = Fixture::new();
    f.tool("factory", DYNAMIC_PROGRAM);
    f.tool("other_factory", "function execute(args,context) margatroid.tools:register({{owner_task_id=margatroid.context.task_id,alias='other_dynamic',description='other',input_schema={type='object'}}}); margatroid.task:return_result('ready'); margatroid.task:receive() end");
    let other = f.execute("other-owner", "other_factory", json!({}));
    f.response(other).unwrap();
    let owner = f.execute("owner", "factory", json!({}));
    f.response(owner).unwrap();
    let returned = f.execute(
        "returned-dependent",
        "dynamic_text",
        json!({"text":"kept","returned":true}),
    );
    f.response(returned).unwrap();
    let mut waiting = f.execute(
        "waiting-dependent",
        "dynamic_number",
        json!({"number":5,"hold":true}),
    );
    let received = f.output("owner");
    assert_eq!(received.output[0]["request_id"], "call-waiting-dependent");
    finish_task(&mut f, "owner");
    assert!(f.tasks().get("waiting-dependent").is_ok());
    assert!(matches!(
        waiting.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert!(tools.get("dynamic_text").is_none());
    assert!(tools.get("dynamic_number").is_none());
    assert!(tools.get("other_dynamic").is_some());
    assert!(tools.get("factory").is_some());
    f.tasks()
        .send("returned-dependent", json!({"after_owner":true}))
        .unwrap();
    assert_eq!(
        f.output("returned-dependent").output,
        vec![json!({"after_owner":true})]
    );
    f.tasks().cancel("waiting-dependent").unwrap();
    assert_eq!(f.response(waiting).unwrap_err().kind, Kind::Cancelled);
    f.removed("waiting-dependent");
    finish_task(&mut f, "returned-dependent");
    finish_task(&mut f, "other-owner");
    assert!(f.tasks().list().is_empty());
}

#[test]
fn explicit_noncaller_owner_and_static_aliases_have_independent_lifetimes() {
    let mut f = Fixture::new();
    f.tool(
        "factory",
        r#"function configure() return {timeout_ms=0} end
        function execute(args,context)
            if margatroid.context.binding ~= nil then
                assert(margatroid.context.owner_task_id == nil)
                margatroid.task:return_result('static survived')
                margatroid.task:finish(margatroid.context.task_id)
            elseif margatroid.context.arguments.owner ~= nil then
                margatroid.tools:register({
                    {alias='bound_alias',description='bound',input_schema={type='object'},owner_task_id=margatroid.context.arguments.owner},
                    {alias='static_alias',description='static',input_schema={type='object'},binding={static=true}}
                })
                margatroid.task:return_result('injected')
                margatroid.task:finish(margatroid.context.task_id)
            else
                margatroid.task:return_result('owner ready')
                margatroid.task:receive()
            end
        end"#,
    );
    let owner = f.execute("A", "factory", json!({}));
    assert_eq!(f.response(owner).unwrap().content, "owner ready");
    let caller = f.execute("C", "factory", json!({"owner":"A"}));
    assert_eq!(f.response(caller).unwrap().content, "injected");
    f.removed("C");
    assert!(f.tasks().get("A").is_ok());
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert_eq!(
        tools.get("bound_alias").unwrap().owner_task_id.as_deref(),
        Some("A")
    );
    assert_eq!(tools.get("static_alias").unwrap().owner_task_id, None);

    finish_task(&mut f, "A");
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert!(tools.get("bound_alias").is_none());
    assert!(tools.get("static_alias").is_some());
    let static_call = f.execute("static-call", "static_alias", json!({}));
    assert_eq!(f.response(static_call).unwrap().content, "static survived");
    f.removed("static-call");
    assert!(f
        .app
        .world()
        .get_component::<Tools>(f.agent)
        .unwrap()
        .get("static_alias")
        .is_some());
    assert!(f.tasks().list().is_empty());
}

#[test]
fn invalid_explicit_owner_rejects_static_and_bound_entries_as_one_batch() {
    let mut f = Fixture::new();
    f.tool(
        "keepalive",
        "function execute() margatroid.task:return_result('ready'); margatroid.task:receive() end",
    );
    let owner = f.execute("finished-owner", "keepalive", json!({}));
    f.response(owner).unwrap();
    finish_task(&mut f, "finished-owner");
    f.tool(
        "factory",
        r#"function execute(args,context)
            local ok,err=pcall(function() margatroid.tools:register({
                {alias='static_alias',description='static',input_schema={type='object'}},
                {alias='bound_alias',description='bound',input_schema={type='object'},owner_task_id=margatroid.context.arguments.owner}
            }) end)
            margatroid.task:return_result({accepted=ok,error=tostring(err)})
            margatroid.task:finish(margatroid.context.task_id)
        end"#,
    );
    for (index, owner) in ["", "missing-owner", "finished-owner"]
        .into_iter()
        .enumerate()
    {
        let caller = format!("invalid-injector-{index}");
        let reply = f.execute(&caller, "factory", json!({"owner":owner}));
        let result: Value = serde_json::from_str(&f.response(reply).unwrap().content).unwrap();
        assert_eq!(result["accepted"], false, "owner {owner:?}");
        f.removed(&caller);
        let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
        assert!(tools.get("static_alias").is_none());
        assert!(tools.get("bound_alias").is_none());
        assert_eq!(tools.iter().count(), 2);
    }
    assert!(f.tasks().list().is_empty());
}

#[test]
fn injection_alias_conflict_rejects_the_whole_batch() {
    let mut f = Fixture::new();
    f.tool(
        "factory",
        r#"function execute(args,context)
        local ok,err=pcall(function() margatroid.tools:register({
            {alias='fresh_alias',description='fresh',input_schema={type='object'}},
            {alias='factory',description='conflict',input_schema={type='object'}}
        }) end)
        margatroid.task:return_result({accepted=ok,error=tostring(err)})
        margatroid.task:receive()
    end"#,
    );
    let reply = f.execute("conflicting-owner", "factory", json!({}));
    let result = serde_json::from_str::<Value>(&f.response(reply).unwrap().content).unwrap();
    assert_eq!(result["accepted"], false);
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert!(tools.get("fresh_alias").is_none());
    assert_eq!(tools.iter().count(), 1);
    finish_task(&mut f, "conflicting-owner");
}

#[test]
fn cancellation_between_injection_dispatch_and_world_commit_leaves_no_aliases() {
    let mut f = Fixture::new();
    f.tool("factory", "function execute(args,context) margatroid.task:append_output('inject-dispatching'); margatroid.tools:register({{owner_task_id=margatroid.context.task_id,alias='cancelled_alias',description='cancelled',input_schema={type='object'}}}); margatroid.task:return_result('unexpected'); margatroid.task:receive() end");
    let reply = f.execute("cancelled-owner", "factory", json!({}));
    // Observe the worker without ticking World, leaving its injection uncommitted.
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.tasks().read("cancelled-owner").unwrap().output.is_empty() {
        assert!(Instant::now() < deadline, "worker never reached injection");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(30));
    assert!(f
        .app
        .world()
        .get_component::<Tools>(f.agent)
        .unwrap()
        .get("cancelled_alias")
        .is_none());
    f.tasks().cancel("cancelled-owner").unwrap();
    assert_eq!(f.response(reply).unwrap_err().kind, Kind::Cancelled);
    f.removed("cancelled-owner");
    let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
    assert!(tools.get("cancelled_alias").is_none());
    assert_eq!(tools.iter().count(), 1);
}

#[test]
fn tool_host_namespaces_reject_legacy_aliases_and_preserve_readonly_context() {
    let mut f = Fixture::new();
    f.tool(
        "contract",
        r#"
function configure(arguments, context)
    assert(context.arguments.value == 'original')
    assert(margatroid.context.arguments.value == 'original')
    return {timeout_ms=30000}
end
function execute(arguments, context)
    assert(context.arguments.value == 'original')
    assert(margatroid.version == nil and margatroid.arguments == nil and margatroid.null == nil)
    assert(margatroid.tool == nil)
    for _, name in ipairs({'read_file','write_file','path_info','path_resolve','directory_list',
        'json_encode','json_decode','yaml_encode','yaml_decode','toml_encode','toml_decode',
        'base64_encode','base64_decode','process_open','await_any','task_return','task_finish',
        'task_write','task_replace','task_read','task_send','task_receive','tool_inject'}) do
        assert(margatroid[name] == nil, 'legacy method remains: '..name)
    end
    for _, operation in ipairs({
        function() margatroid.driver:mcl('EFFECT ready') end,
        function() margatroid.driver:wait() end,
        function() margatroid.context={} end,
        function() margatroid.files={} end,
        function() margatroid.data.null={} end,
        function() margatroid.task.finish={} end,
        function() margatroid.tools.register={} end,
    }) do assert(not pcall(operation)) end
    local copy=margatroid.context
    copy.arguments.value='forged'
    copy.task_id='forged'
    assert(margatroid.context.arguments.value == 'original')
    assert(margatroid.context.task_id == 'contract')
    assert(margatroid.data.null ~= nil)
    margatroid.task:return_result('contract verified')
end
"#,
    );
    let reply = f.execute("contract", "contract", json!({"value":"original"}));
    assert_eq!(f.response(reply).unwrap().content, "contract verified");
    f.removed("contract");
}
