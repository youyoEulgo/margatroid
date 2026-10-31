#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::{Agent, AgentState};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::{App, Entity};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolError, ToolErrorKind as Kind, ToolExecuteRequest, ToolExecuteResponse,
    ToolExecutionLimits, ToolTasks,
};
use tool_plugin::ToolPlugin;
use tool_resource_core::Tools;
use workspace_core::{Workspace, WorkspaceState};

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(ToolExecutionLimits {
            execution_timeout: Some(Duration::from_secs(5)),
            host_call_timeout: Duration::from_secs(2),
            cleanup_timeout: Duration::from_secs(1),
            ..Default::default()
        })
    }

    fn with_limits(limits: ToolExecutionLimits) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(
                ToolPlugin::new()
                    .with_runner_path(env!("CARGO_BIN_EXE_tool_runner"))
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
        Self { app, agent, root }
    }

    fn tool(&mut self, alias: &str, program: &str) {
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
        receiver
    }

    fn response(
        &mut self,
        mut reply: oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
    ) -> Result<ToolExecuteResponse, ToolError> {
        let deadline = Instant::now() + Duration::from_secs(10);
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
        shell("setsid /bin/sh -c 'echo $$ > escaped.pid; exec sleep 60' </dev/null >/dev/null 2>&1 &")
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
        execution_timeout: Some(Duration::from_secs(3)),
        host_call_timeout: Duration::from_secs(1),
        ..Default::default()
    });
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
fn execution_timeout_still_reaps_a_background_vm_after_return() {
    let mut fixture = Fixture::with_limits(ToolExecutionLimits {
        execution_timeout: Some(Duration::from_millis(200)),
        host_call_timeout: Duration::from_millis(100),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    });
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
            json!({"text":"sleep 60 & disown; exit 0", "enter":true}),
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
fn an_unlimited_background_task_can_wait_for_input_beyond_host_call_timeout() {
    let mut fixture = Fixture::with_limits(ToolExecutionLimits {
        execution_timeout: None,
        host_call_timeout: Duration::from_millis(200),
        ..Default::default()
    });
    fixture.tool("waiting", "function execute(arguments, context) task_return(context.task_id); local value=task_receive(); task_write(value); task_finish(context.task_id) end");
    let reply = fixture.execute("waiting", "waiting", json!({}));
    fixture.response(reply).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!fixture.tasks().get("waiting").unwrap().is_completed());
    fixture
        .tasks()
        .send("waiting", json!({"continued":true}))
        .unwrap();
    fixture.completed("waiting");
    assert_eq!(
        fixture.tasks().read("waiting").unwrap().output,
        vec![json!({"continued":true})]
    );
    assert!(ToolExecutionLimits::default().execution_timeout.is_none());
}
