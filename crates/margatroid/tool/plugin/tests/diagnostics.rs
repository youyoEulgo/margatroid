#![cfg(target_os = "linux")]

#[path = "support/storage.rs"]
mod storage_fixture;

use agent_core::{Agent, AgentState};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_core::{AgentLogLevel, AgentLogSource, AgentStorage, StoredAgentLog};
use tokio::sync::oneshot;
use tool_core::{InitToolTasks, ToolError, ToolExecuteRequest, ToolExecuteResponse, ToolTasks};
use tool_plugin::ToolPlugin;
use tool_resource_core::{read_tool, ToolEntry, Tools};
use workspace_core::{Workspace, WorkspaceState};

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(ToolPlugin::new().with_runner_path(env!("CARGO_BIN_EXE_tool_runner")));
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                name: "diagnostics".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().to_owned(),
                state: WorkspaceState::Running,
                agents: Default::default(),
            },
        );
        let agent = app.world_mut().spawn();
        app.world_mut().insert_component(
            agent,
            Agent {
                name: "diagnostic-agent".into(),
                workspace,
                state: AgentState::Working,
                turn: Some("diagnostic-turn".into()),
            },
        );
        storage_fixture::mount(app.world_mut(), agent, root.path());
        app.world_mut().insert_component(agent, Tools::default());
        let (sender, mut receiver) = oneshot::channel();
        app.world().emit_event(InitToolTasks {
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        app.tick();
        receiver.try_recv().unwrap().unwrap();
        Self { app, agent, root }
    }

    fn execute(
        &mut self,
        alias: &str,
        program: &str,
    ) -> oneshot::Receiver<Result<ToolExecuteResponse, ToolError>> {
        let directory = self.root.path().join(alias);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("tool.toml"),
            "schema_version=1\ndescription='diagnostics test'\n",
        )
        .unwrap();
        std::fs::write(directory.join("input.schema.json"), "{}").unwrap();
        std::fs::write(directory.join("main.lua"), program).unwrap();
        let imported = read_tool(&directory.join("tool.toml")).unwrap();
        self.app
            .world_mut()
            .get_component_mut::<Tools>(self.agent)
            .unwrap()
            .insert(
                alias.into(),
                ToolEntry {
                    path: imported.origin,
                    description: imported.description,
                    input_schema: imported.input_schema,
                    owner_task_id: None,
                    binding: None,
                },
            )
            .unwrap();
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(ToolExecuteRequest {
            id: alias.into(),
            agent: self.agent,
            turn: "diagnostic-turn".into(),
            alias: alias.into(),
            call_id: format!("call-{alias}"),
            arguments: json!({}),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
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
                Err(error) => panic!("reply closed: {error}"),
            }
            assert!(Instant::now() < deadline, "tool did not reply");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn drained(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self
            .app
            .world()
            .get_component::<ToolTasks>(self.agent)
            .unwrap()
            .list()
            .is_empty()
        {
            self.app.tick();
            assert!(Instant::now() < deadline, "tool did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn logs(&self) -> Vec<StoredAgentLog> {
        storage_core::read_agent_logs(
            self.app
                .world()
                .get_component::<AgentStorage>(self.agent)
                .unwrap(),
            None,
            100,
        )
        .unwrap()
    }
}

fn assert_brief(message: &str) {
    assert!(!message.contains("stack traceback"), "{message}");
    assert!(!message.contains("main.lua:"), "{message}");
    assert!(!message.contains("[string"), "{message}");
}

fn assert_association(record: &storage_core::AgentLogRecord, alias: &str) {
    let context = record.context_json.as_ref().unwrap();
    assert_eq!(context["agent"], "diagnostic-agent");
    assert_eq!(context["turn"], "diagnostic-turn");
    assert_eq!(context["task_id"], alias);
    assert_eq!(context["call_id"], format!("call-{alias}"));
    assert_eq!(context["alias"], alias);
    assert!(record.origin.as_ref().unwrap().contains(alias));
}

#[test]
fn syntax_loading_configuration_and_execution_errors_keep_raw_diagnostics_out_of_reply() {
    for (alias, program, stage, marker) in [
        ("syntax", "function execute( end", "load", "syntax"),
        (
            "loading",
            "error('private-loading-marker')",
            "load",
            "private-loading-marker",
        ),
        (
            "configuration",
            "function configure() error('private-config-marker') end function execute() end",
            "configure",
            "private-config-marker",
        ),
        (
            "execution",
            "function configure() return {timeout_ms=30000} end function execute() error('private-execute-marker') end",
            "execute",
            "private-execute-marker",
        ),
    ] {
        let mut f = Fixture::new();
        let reply = f.execute(alias, program);
        let failure = f.response(reply).unwrap_err();
        assert_brief(&failure.message);
        f.drained();
        let logs = f.logs();
        assert_eq!(logs.len(), 1, "{alias}: {logs:?}");
        let record = &logs[0].record;
        assert_eq!(record.level, AgentLogLevel::Error);
        assert_eq!(record.source, AgentLogSource::Tool);
        assert_eq!(record.stage, stage);
        let raw = record.raw_error.as_deref().unwrap();
        assert!(raw.contains(marker), "{alias}: {raw}");
        assert!(raw.contains(":1:"), "raw error must retain line: {raw}");
        assert_association(record, alias);
        assert_eq!(
            storage_core::execute_storage(
                f.app
                    .world()
                    .get_component::<AgentStorage>(f.agent)
                    .unwrap(),
                storage_core::StorageOperation::ReadHistoryEntries
            )
            .unwrap(),
            storage_core::StorageValue::HistoryEntries(vec![])
        );
    }
}

#[test]
fn explicit_logging_preserves_details_and_validates_permissions_and_levels() {
    let mut f = Fixture::new();
    let reply = f.execute("active-log", r#"
function configure() return {timeout_ms=30000} end
function execute()
    for _, level in ipairs({'debug','info','warn','error'}) do
        margatroid:log(level, 'visible diagnostic', {unicode='日志', nested={answer=42}, agent='forged'})
    end
    local ok = pcall(function() margatroid:log('fatal', 'invalid') end)
    assert(not ok)
    margatroid.task:return_result('logged')
end
"#);
    assert_eq!(f.response(reply).unwrap().content, "logged");
    f.drained();
    let logs = f.logs();
    assert_eq!(logs.len(), 4);
    for (entry, level) in logs.iter().zip([
        AgentLogLevel::Debug,
        AgentLogLevel::Info,
        AgentLogLevel::Warn,
        AgentLogLevel::Error,
    ]) {
        assert_eq!(entry.record.level, level);
        assert_eq!(entry.record.source, AgentLogSource::Tool);
        assert_eq!(
            entry.record.details_json,
            Some(json!({"unicode":"日志","nested":{"answer":42},"agent":"forged"}))
        );
        assert_eq!(entry.record.raw_error, None);
        assert_association(&entry.record, "active-log");
    }

    let reply = f.execute("configure-log", "function configure() margatroid:log('info','forbidden') return {timeout_ms=30000} end function execute() end");
    assert_brief(&f.response(reply).unwrap_err().message);
    f.drained();
    let logs = f.logs();
    assert_eq!(logs.len(), 5);
    assert_eq!(logs[4].record.stage, "configure");
    assert_eq!(logs[4].record.level, AgentLogLevel::Error);
}

#[test]
fn error_after_return_keeps_original_reply_and_records_once() {
    let mut f = Fixture::new();
    let reply = f.execute("returned", "function configure() return {timeout_ms=30000} end function execute() margatroid.task:return_result('original reply'); error('private-after-return-marker') end");
    let response = f.response(reply).unwrap();
    assert!(!response.failed);
    assert_eq!(response.content, "original reply");
    f.drained();
    let logs = f.logs();
    assert_eq!(logs.len(), 1);
    assert!(logs[0]
        .record
        .raw_error
        .as_deref()
        .unwrap()
        .contains("private-after-return-marker"));
    assert_association(&logs[0].record, "returned");
}

#[test]
fn tool_rejects_driver_namespace_and_removed_task_aliases_with_tool_source_logs() {
    for (alias, operation) in [
        ("driver-mcl", "margatroid.driver:mcl('GET x')"),
        ("driver-wait", "margatroid.driver:wait()"),
        ("removed-alias", "margatroid:task_return('old alias')"),
    ] {
        let mut f = Fixture::new();
        let reply = f.execute(alias, &format!("function configure() return {{timeout_ms=30000}} end function execute() {operation} end"));
        assert_brief(&f.response(reply).unwrap_err().message);
        f.drained();
        let logs = f.logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].record.source, AgentLogSource::Tool);
        assert_association(&logs[0].record, alias);
    }
}

#[test]
fn missing_agent_storage_rejects_tool_before_registering_a_task() {
    let mut f = Fixture::new();
    f.app.world_mut().remove_component::<AgentStorage>(f.agent);
    let reply = f.execute("missing-storage", "function configure() return {timeout_ms=30000} end function execute() margatroid.task:return_result('unsafe') end");
    let failure = f.response(reply).unwrap_err();
    assert!(failure.message.contains("storage") || failure.message.contains("Storage"));
    assert!(f
        .app
        .world()
        .get_component::<ToolTasks>(f.agent)
        .unwrap()
        .list()
        .is_empty());
}
