#![cfg(target_os = "linux")]

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
use tool_resource_core::Tools;
use workspace_core::{Workspace, WorkspaceState};

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
                name: "test".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().to_owned(),
                state: WorkspaceState::Running,
                agents: BTreeSet::new(),
            },
        );
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            Agent {
                name: "test".into(),
                workspace,
                state: AgentState::Working,
                turn: None,
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
            .insert(alias.into(), directory.join("tool.toml"));
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

    fn tasks(&self) -> ToolTasks {
        self.app
            .world()
            .get_component::<ToolTasks>(self.agent)
            .unwrap()
            .clone()
    }

    fn completed(&self, id: &str) {
        let handle = self.tasks().get(id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_completed() {
            assert!(
                Instant::now() < deadline,
                "task {id} not completed: {:?}",
                handle.read().error
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn output(&self, id: &str) -> tool_core::TaskReadResult {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
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
        assert!(!handle.is_completed(), "{mode}: unproven cleanup completed");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if mode != "cancel" {
            let finish = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), handle.finish()).await
            });
            assert_eq!(finish.unwrap().unwrap_err().kind, Kind::CleanupFailed);
        } else {
            assert_eq!(handle.read().error.unwrap().kind, Kind::Cancelled);
        }
        assert!(!handle.is_completed(), "{mode}: finish dropped supervision");
        assert!(fixture.tasks().get("pending-cleanup").is_ok());
        drop(release);
        fixture.completed("pending-cleanup");
        runtime.block_on(handle.finish()).unwrap();
    }
}

#[test]
fn short_return_then_self_finish_stops_even_caught_finish_control() {
    let mut fixture = Fixture::new();
    fixture.tool("short", "function execute(args, context) task_return('hello'); pcall(function() task_finish(context.task_id) end); task_write('unreachable') end");
    let reply = fixture.execute("short-task", "short", json!({}));
    let response = fixture.response(reply).unwrap();
    assert_eq!(response.content, "hello");
    assert_eq!(response.call_id, "call-short-task");
    assert_eq!(response.alias, "short");
    fixture.completed("short-task");
    let result = fixture.tasks().read("short-task").unwrap();
    assert!(result.completed);
    assert!(result.output.is_empty());
    assert_eq!(result.error, None);
}

#[test]
fn return_keeps_vm_alive_across_turn_changes_and_supports_json_cache_replacement() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "background",
        r#"function execute(args, context)
        task_return({task_id=context.task_id})
        while true do
            local input = task_receive()
            if input.finish then task_finish(context.task_id) end
            if input.replace then task_replace(input.value) else task_write(input) end
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
        .is_completed());
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
    assert!(!first.completed);
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
    fixture.completed("background-task");
}

#[test]
fn finish_other_task_waits_for_real_cleanup_and_retains_both_entries() {
    let mut fixture = Fixture::new();
    fixture.tool("background", "function execute(args, context) task_return('started'); task_receive(); task_write('unexpected') end");
    let reply = fixture.execute("target", "background", json!({}));
    fixture.response(reply).unwrap();
    fixture.tool("closer", "function execute(args, context) task_finish(args.target); task_return('closed'); task_finish(context.task_id) end");
    let reply = fixture.execute("closer", "closer", json!({"target":"target"}));
    assert_eq!(fixture.response(reply).unwrap().content, "closed");
    assert!(fixture.tasks().get("target").unwrap().is_completed());
    fixture.completed("closer");
    assert_eq!(fixture.tasks().list().len(), 2);
    assert!(fixture.tasks().read("target").unwrap().output.is_empty());
}

#[test]
fn lua_can_send_receive_and_read_another_same_agent_task() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "echo",
        r#"function execute(args, context)
        task_return('ready')
        local input=task_receive()
        task_write(input.value)
        task_send(input.reply_to, {ready=true})
        task_receive()
    end"#,
    );
    let reply = fixture.execute("echo-task", "echo", json!({}));
    fixture.response(reply).unwrap();
    fixture.tool(
        "reader",
        r#"function execute(args, context)
        task_send(args.target, {value={1,true,json.null}, reply_to=context.task_id})
        task_receive()
        local result=task_read(args.target)
        task_finish(args.target)
        task_return(result)
        task_finish(context.task_id)
    end"#,
    );
    let reply = fixture.execute("reader-task", "reader", json!({"target":"echo-task"}));
    let response = fixture.response(reply).unwrap();
    let result: Value = serde_json::from_str(&response.content).unwrap();
    assert_eq!(result["output"], json!([[1, true, null]]));
    assert_eq!(result["completed"], false);
    fixture.completed("reader-task");
    fixture.completed("echo-task");
    assert!(fixture.tasks().read("echo-task").unwrap().output.is_empty());
}

#[test]
fn natural_exit_without_return_reports_missing_return_and_late_errors_are_queryable() {
    let mut fixture = Fixture::new();
    fixture.tool("missing", "function execute() return 'not task_return' end");
    let reply = fixture.execute("missing", "missing", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::MissingReturn
    );
    fixture.completed("missing");
    fixture.tool(
        "late",
        "function execute() task_return('once'); task_return('twice') end",
    );
    let reply = fixture.execute("late", "late", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "once");
    fixture.completed("late");
    assert_eq!(
        fixture.tasks().read("late").unwrap().error.unwrap().kind,
        Kind::AlreadyReturned
    );
}

#[test]
fn fresh_schema_program_and_deleted_files_are_checked_for_each_execution() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "fresh",
        "function execute(args,context) task_return('first'); task_finish(context.task_id) end",
    );
    let reply = fixture.execute("first", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "first");
    fixture.completed("first");
    fixture.tool(
        "fresh",
        "function execute(args,context) task_return('changed'); task_finish(context.task_id) end",
    );
    let reply = fixture.execute("second", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "changed");
    fixture.completed("second");
    std::fs::write(
        fixture.root.path().join("fresh/input.schema.json"),
        r#"{"type":"object","required":["required"]}"#,
    )
    .unwrap();
    let reply = fixture.execute("bad-args", "fresh", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::InvalidArguments
    );
    fixture.completed("bad-args");
    let path = fixture.root.path().join("fresh/main.lua");
    assert_eq!(
        path.parent(),
        Some(fixture.root.path().join("fresh").as_path())
    );
    std::fs::remove_file(path).unwrap();
    let reply = fixture.execute("missing-file", "fresh", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::ReadFailed);
    fixture.completed("missing-file");
}

#[test]
fn independent_supervisor_reaps_setsid_descendants_after_return_and_finish() {
    let mut fixture = Fixture::new();
    fixture.tool("escape", r#"function execute(args,context)
        shell("setsid /bin/sh -c 'echo $$ > escaped.pid; exec sleep 15' </dev/null >/dev/null 2>&1 &")
        shell("while [ ! -s escaped.pid ]; do sleep 0.01; done")
        task_return('spawned')
        task_finish(context.task_id)
    end"#);
    let reply = fixture.execute("escape", "escape", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "spawned");
    fixture.completed("escape");
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
        "function execute() task_return('started'); task_receive() end",
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
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::Closed);
    assert!(!fixture.tasks().get("background").unwrap().is_completed());
    fixture.tasks().cancel("background").unwrap();
    fixture.completed("background");
}

#[test]
fn reply_receiver_drop_does_not_cancel_a_registered_task() {
    let mut fixture = Fixture::new();
    fixture.tool("detached", "function execute(args,context) task_return('ignored'); task_write('alive'); task_receive(); task_finish(context.task_id) end");
    let reply = fixture.execute("detached", "detached", json!({}));
    drop(reply);
    assert_eq!(fixture.output("detached").output, vec![json!("alive")]);
    assert!(!fixture.tasks().get("detached").unwrap().is_completed());
    fixture.tasks().send("detached", json!(null)).unwrap();
    fixture.completed("detached");
}

#[test]
fn cache_text_and_instruction_budgets_fail_explicitly() {
    let mut fixture = Fixture::with_limits(ToolExecutionLimits {
        cache_bytes: 8,
        output_bytes: 8,
        instruction_budget: 10000,
        ..Default::default()
    });
    fixture.configuration = ToolConfiguration { timeout_ms: 3_000 };
    fixture.tool(
        "cache",
        "function execute() task_return('ready'); task_write('value-larger-than-cache') end",
    );
    let reply = fixture.execute("cache", "cache", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "ready");
    fixture.completed("cache");
    let result = fixture.tasks().read("cache").unwrap();
    assert_eq!(result.error.unwrap().kind, Kind::OutputTooLarge);
    assert!(result.output.is_empty());
    fixture.tool(
        "text",
        "function execute() task_return('text-larger-than-budget') end",
    );
    let reply = fixture.execute("text", "text", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::OutputTooLarge
    );
    fixture.completed("text");
    fixture.tool("instructions", "function execute() while true do end end");
    let reply = fixture.execute("instructions", "instructions", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::ExecutionFailed
    );
    fixture.completed("instructions");
}

#[test]
fn result_images_are_validated_by_the_core_across_the_runner_protocol() {
    let mut fixture = Fixture::new();
    fixture.tool("image", "function execute(args, context) task_return({ok=true,content='image',images={{media_type='image/png',data='iVBORw0KGgo='}}}); task_finish(context.task_id) end");
    let reply = fixture.execute("image", "image", json!({}));
    let result = fixture.response(reply).unwrap();
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].media_type, "image/png");
    fixture.completed("image");
    fixture.tool("invalid-image", "function execute() task_return({ok=true,content='image',images={{media_type='image/png',data='eA=='}}}) end");
    let reply = fixture.execute("invalid-image", "invalid-image", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::RunnerProtocolFailed
    );
    fixture.completed("invalid-image");
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
    fixture.completed("sleep-eleven");
    assert!(fixture
        .tasks()
        .read("sleep-eleven")
        .unwrap()
        .error
        .is_none());
}

#[test]
fn shell_obeys_only_the_task_lifetime_including_unlimited() {
    for timeout_ms in [0, 3_000] {
        let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms });
        fixture.tool("slow-shell", "function execute(args, context) local result=shell('sleep 0.25; printf done'); task_return(result.stdout); task_finish(context.task_id) end");
        let reply = fixture.execute("slow-shell", "slow-shell", json!({}));
        assert_eq!(fixture.response(reply).unwrap().content, "done");
        fixture.completed("slow-shell");
        assert!(fixture.tasks().read("slow-shell").unwrap().error.is_none());
    }
}

#[test]
fn task_lifetime_reports_timeout_after_real_descendant_cleanup() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 250 });
    fixture.tool(
        "shell-timeout",
        r#"function execute()
        shell("echo $$ > shell.pid; sleep 15 & echo $! > child.pid; wait")
        task_return('unexpected success')
    end"#,
    );
    let reply = fixture.execute("shell-timeout", "shell-timeout", json!({}));
    let error = fixture.response(reply).unwrap_err();
    assert_eq!(error.kind, Kind::Timeout);
    fixture.completed("shell-timeout");
    assert_eq!(
        fixture
            .tasks()
            .read("shell-timeout")
            .unwrap()
            .error
            .unwrap()
            .kind,
        Kind::Timeout
    );
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
        r#"function execute()
        shell("echo $$ > total-shell.pid; exec sleep 15")
        task_return('unexpected success')
    end"#,
    );
    let reply = fixture.execute("total-timeout", "total-timeout", json!({}));
    assert_eq!(fixture.response(reply).unwrap_err().kind, Kind::Timeout);
    fixture.completed("total-timeout");
    assert_eq!(
        fixture
            .tasks()
            .read("total-timeout")
            .unwrap()
            .error
            .unwrap()
            .kind,
        Kind::Timeout
    );
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
fn invalid_shell_timeouts_are_rejected_before_spawning_a_child() {
    let mut fixture = Fixture::new();
    for (index, timeout) in ["0", "250", "-1", "0.5", "math.huge", "'250'", "true"]
        .iter()
        .enumerate()
    {
        let id = format!("invalid-timeout-{index}");
        fixture.tool(
            &id,
            &format!(
                "function execute() shell('touch unexpected-child', {{timeout_ms={timeout}}}) end"
            ),
        );
        let reply = fixture.execute(&id, &id, json!({}));
        assert_eq!(
            fixture.response(reply).unwrap_err().kind,
            Kind::InvalidArguments,
            "{timeout}"
        );
        fixture.completed(&id);
        assert!(!fixture.root.path().join("unexpected-child").exists());
    }
}

#[test]
fn execution_timeout_still_reaps_a_background_vm_after_return() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 200 });
    fixture.tool(
        "timeout",
        "function execute() task_return('started'); task_receive() end",
    );
    let reply = fixture.execute("timeout", "timeout", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "started");
    fixture.completed("timeout");
    assert_eq!(
        fixture.tasks().read("timeout").unwrap().error.unwrap().kind,
        Kind::Timeout
    );
}

#[test]
fn selected_sandbox_is_read_fresh_and_invalid_policy_never_starts_tool() {
    let mut fixture = Fixture::new();
    fixture.tool("safe", "function execute(args,context) task_write('must-not-run'); task_return('unsafe'); task_finish(context.task_id) end");
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
    fixture.completed("safe");
    assert!(fixture.tasks().read("safe").unwrap().output.is_empty());
}

#[test]
fn duplicate_task_id_does_not_replace_the_existing_shared_handle() {
    let mut fixture = Fixture::new();
    fixture.tool("background", "function execute(args,context) task_return('first'); task_receive(); task_finish(context.task_id) end");
    let reply = fixture.execute("same", "background", json!({}));
    fixture.response(reply).unwrap();
    let reply = fixture.execute("same", "background", json!({}));
    assert_eq!(
        fixture.response(reply).unwrap_err().kind,
        Kind::DuplicateTask
    );
    assert_eq!(fixture.tasks().list().len(), 1);
    fixture.tasks().send("same", json!({})).unwrap();
    fixture.completed("same");
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
    assert!(!fixture.tasks().get("terminal").unwrap().is_completed());
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
    fixture.completed("terminal");
    assert!(fixture.tasks().read("terminal").unwrap().completed);
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
        fixture.completed("history");
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
    fixture.completed("screen");
}

#[test]
fn a_full_task_mailbox_does_not_block_the_original_call_reply() {
    let mut fixture = Fixture::new();
    fixture.tool(
        "mailbox",
        r#"function execute(arguments, context)
        shell("sleep 0.1")
        task_return({ ready = true })
        while true do
            local value = task_receive()
            if value == "stop" then break end
            task_write(value)
        end
        task_finish(context.task_id)
    end"#,
    );
    let reply = fixture.execute("mailbox", "mailbox", json!({}));
    let mut accepted = 0;
    for index in 0..1000 {
        match fixture.tasks().send("mailbox", json!(index)) {
            Ok(()) => accepted += 1,
            Err(error) => assert_eq!(error.kind, Kind::InputFull),
        }
    }
    assert!(accepted > 0 && accepted < 1000);
    assert_eq!(fixture.response(reply).unwrap().content, "{\"ready\":true}");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match fixture.tasks().send("mailbox", json!("stop")) {
            Ok(()) => break,
            Err(error) => assert_eq!(error.kind, Kind::InputFull),
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture.completed("mailbox");
    assert_eq!(
        fixture.tasks().read("mailbox").unwrap().output.len(),
        accepted
    );
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
    fixture.completed("background-child");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(fixture
        .tasks()
        .read("background-child")
        .unwrap()
        .error
        .is_none());
}

#[test]
fn unlimited_task_waits_for_input_and_other_task_finish_proves_cleanup() {
    let mut fixture = Fixture::with_configuration(ToolConfiguration { timeout_ms: 0 });
    fixture.tool("waiting", "function execute(arguments, context) task_return(context.task_id); local value=task_receive(); task_write(value); task_receive() end");
    let reply = fixture.execute("waiting", "waiting", json!({}));
    fixture.response(reply).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!fixture.tasks().get("waiting").unwrap().is_completed());
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
        "function execute() task_finish('waiting'); task_return('closed') end",
    );
    let reply = fixture.execute("closer-unlimited", "closer-unlimited", json!({}));
    assert_eq!(fixture.response(reply).unwrap().content, "closed");
    fixture.completed("waiting");
    assert!(fixture.tasks().read("waiting").unwrap().completed);
}

fn with_daemon_controlling_terminal(name: &str, test: impl FnOnce()) {
    const CHILD: &str = "MARGATROID_CONTROLLING_TERMINAL_TEST";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let tty = std::fs::File::open("/dev/tty").expect("daemon must have a controlling terminal");
        assert_eq!(
            nix::unistd::tcgetpgrp(&tty).unwrap(),
            nix::unistd::getpgrp()
        );
        assert_eq!(nix::unistd::getsid(None).unwrap(), nix::unistd::getpid());
        test();
        return;
    }
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
                r#"function execute(args, context)
                local result = shell([=[python3 - <<'PY'
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
                task_return(result)
                task_finish(context.task_id)
            end"#,
            );
            let reply = fixture.execute("raw-mode", "raw-mode", json!({}));
            let response = fixture.response(reply).unwrap();
            let result: Value = serde_json::from_str(&response.content).unwrap();
            assert_eq!(result["exit_code"], 0, "{result}");
            assert_eq!(result["stdout"], "NO_CONTROLLING_TERMINAL:ENXIO\n");
            fixture.completed("raw-mode");
            assert!(fixture.tasks().read("raw-mode").unwrap().error.is_none());
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
                r#"function execute(args, context)
                local result = shell([=[python3 - <<'PY'
import json, os
worker = os.getpid()
while True:
    with open('/proc/%d/stat' % worker) as source:
        parent = int(source.read().rsplit(')', 1)[1].split()[1])
    if parent == os.getpgrp():
        break
    worker = parent
print(json.dumps(dict(group=os.getpgrp(), worker=worker)))
PY]=])
                task_return(result.stdout)
                task_receive()
                task_finish(context.task_id)
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
            fixture.completed("stopped");
            assert!(started.elapsed() < Duration::from_secs(3));
            assert_eq!(
                fixture.tasks().read("stopped").unwrap().error.unwrap().kind,
                Kind::Cancelled
            );
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
            fixture.completed("own-pty");
            assert!(fixture.tasks().read("own-pty").unwrap().error.is_none());
        },
    );
}
