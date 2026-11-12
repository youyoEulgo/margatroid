use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity};
use lua_runtime_core::DriverLuaVm;
use mcl_core::{MclCommand, MclInstruction, MclValue, MessageArray};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage_core::{
    AgentStorage, StorageOperation, StorageValue, StoredHistoryRecord, execute_storage,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{RestartWorkspace, StartWorkspace, WorkspaceOperationOutput};

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn wait_workspace(app: &mut App, id: &str, _restart: bool) -> WorkspaceOperationOutput {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        let result = poll_reply(id);
        if let Some(result) = result {
            let output = result.unwrap();
            assert!(
                output.member_errors.is_empty(),
                "{:?}",
                output.member_errors
            );
            assert_eq!(output.members.len(), 1);
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "workspace operation {id} timed out"
        );
        std::thread::yield_now();
    }
}

fn command(app: &mut App, agent: Entity, text: &str, arguments: Vec<Value>) -> MclValue {
    let instance = app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .instance
        .start_id
        .clone();
    let (sender, mut receiver) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("block-files:{text}"),
        source: agent,
        source_address: app
            .world()
            .get_component::<address::Address>(agent)
            .unwrap()
            .as_str()
            .to_owned(),
        source_instance: Some(instance),
        target: agent,
        instruction: MclInstruction {
            text: text.into(),
            arguments,
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Ok(result) = receiver.try_recv() {
            return result.unwrap();
        }
        assert!(Instant::now() < deadline, "MCL command {text} timed out");
        std::thread::yield_now();
    }
}

fn entries(app: &App, agent: Entity, operation: StorageOperation) -> Vec<StoredHistoryRecord> {
    let storage = app.world().get_component::<AgentStorage>(agent).unwrap();
    let StorageValue::HistoryEntries(entries) = execute_storage(storage, operation).unwrap() else {
        panic!("expected stored history entries");
    };
    entries
}

fn assert_disk_schema_and_messages(path: &Path, message: &Value, log_count: usize) {
    let db = rusqlite::Connection::open(path).unwrap();
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "agent_logs",
            "agent_messages",
            "block_snapshots",
            "mcl_records"
        ]
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM agent_logs", [], |row| row
            .get::<_, usize>(0))
            .unwrap(),
        log_count
    );
    let columns = db
        .prepare("PRAGMA table_info(agent_messages)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "sequence",
            "turn",
            "role",
            "content",
            "data",
            "tool_call",
            "payload",
            "created_at_ms"
        ]
    );
    let (role, content): (String, String) = db
        .query_row(
            "SELECT role,content FROM agent_messages LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(role, message["type"].as_str().unwrap());
    assert_eq!(content, message["content"].as_str().unwrap());
    let mappings = db.prepare("SELECT block_name,instance,kind,snapshot_id,file_name,fields_json,upstream_id,restore_intent FROM block_snapshots")
        .unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, i64>(3)?,row.get::<_, String>(4)?,row.get::<_, String>(5)?,row.get::<_, Option<i64>>(6)?,row.get::<_, Option<String>>(7)?)))
        .unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(
        mappings.len(),
        1,
        "TEMP Blocks must never have disk mappings"
    );
    let (name, instance, kind, snapshot_id, file_name, fields_json, upstream, intent) =
        &mappings[0];
    assert_eq!(name, "session");
    assert!(instance.is_empty());
    assert_eq!(kind, "timeline");
    assert_eq!(*snapshot_id, 0);
    assert_eq!(file_name, "session/timeline.sqlite");
    assert!(upstream.is_none() && intent.is_none());
    let block_root = std::path::PathBuf::from(format!("{}.blocks", path.display()));
    let directories = std::fs::read_dir(&block_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        directories,
        [std::ffi::OsString::from("session")],
        "TEMP Block files or namespaces must not exist"
    );
    let block = rusqlite::Connection::open(block_root.join(file_name)).unwrap();
    assert_eq!(
        block
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let fields: Value = serde_json::from_str(fields_json).unwrap();
    assert_eq!(fields.as_object().unwrap().len(), 1);
    let table = fields["conversation"].as_str().unwrap();
    assert!(
        table
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    );
    let tables = block
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(tables, [table]);
    let columns = block
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "position",
            "sequence",
            "role",
            "content",
            "data",
            "tool_call",
            "payload"
        ]
    );
    let stored: (String, String) = block
        .query_row(
            &format!("SELECT role,content FROM {table} ORDER BY position"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored.0, message["type"].as_str().unwrap());
    assert_eq!(stored.1, message["content"].as_str().unwrap());
    assert!(path.parent().unwrap().join("setting.toml").is_file());
    assert!(!path.parent().unwrap().join("settings.toml").exists());
}

#[test]
fn workspace_restarts_keep_block_files_persistent_context_and_history_without_temp_file_growth() {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources"),
        root.path(),
    );
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let directory = root.path().join("workspaces/four-table");
    std::fs::create_dir_all(&directory).unwrap();
    let definition = directory.join("workspace.yaml");
    std::fs::write(&definition, format!(
        "name: four-table\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/coder:latest\n    home_root: {}\n",
        project.display(), root.path().display())).unwrap();
    std::fs::write(root.path().join("config.toml"),
        "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:unmatched']\nbackend_state=['type:unmatched']\n").unwrap();
    let driver_model = agent_driver_core::read_driver(
        &root
            .path()
            .join("agent-drivers/local/coder/latest/driver.toml"),
        &Default::default(),
    )
    .unwrap()
    .manifest
    .inference
    .model;
    std::fs::write(root.path().join("models.toml"),
        format!("[[models]]\nid={driver_model:?}\nmodel='unused'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused-local-test'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n")).unwrap();
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().to_owned(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    app.world().emit_event(StartWorkspace {
        id: "initial".into(),
        reply: request_reply("initial"),
        path: definition.clone(),
    });
    let mut output = wait_workspace(&mut app, "initial", false);
    let message = json!({"type":"user", "content":"persistent context and history"});
    let agent = output.members[0];
    command(
        &mut app,
        agent,
        "INJECT ? TO session.conversation",
        vec![json!([message.clone()])],
    );
    command(
        &mut app,
        agent,
        "EFFECT history_append ?",
        vec![json!([message.clone()])],
    );
    let expected_messages = entries(&app, agent, StorageOperation::ReadAgentMessages);
    assert_eq!(expected_messages.len(), 1);
    let mut previous_history = entries(&app, agent, StorageOperation::ReadHistoryEntries);
    let database_path = directory.join("agents/coder/state.sqlite");
    let mut instances = std::collections::BTreeSet::new();
    let mut expected_logs = Vec::new();
    for cycle in 0..4 {
        let agent = output.members[0];
        let instance = app
            .world()
            .get_component::<DriverLuaVm>(agent)
            .unwrap()
            .instance
            .start_id
            .clone();
        assert!(instances.insert(instance.clone()));
        assert_eq!(
            command(&mut app, agent, "GET session.conversation", vec![]),
            MclValue::Array(MessageArray::Message(vec![message.clone()]))
        );
        command(
            &mut app,
            agent,
            "CREATE TEMP BLOCK scratch (messages)",
            vec![],
        );
        assert_eq!(
            command(&mut app, agent, "GET scratch.messages", vec![]),
            MclValue::Array(MessageArray::Message(vec![]))
        );
        let temporary =
            json!({"type":"user", "content":format!("temporary source from cycle {cycle}")});
        command(
            &mut app,
            agent,
            "INJECT ? TO scratch.messages",
            vec![json!([temporary.clone()])],
        );
        assert_eq!(
            command(&mut app, agent, "GET scratch.messages", vec![]),
            MclValue::Array(MessageArray::Message(vec![temporary]))
        );
        let storage = app.world().get_component::<AgentStorage>(agent).unwrap();
        assert_eq!(
            storage_core::read_agent_logs(storage, None, 100).unwrap(),
            expected_logs
        );
        let history_before_log = entries(&app, agent, StorageOperation::ReadHistoryEntries);
        let saved = storage_core::append_agent_log(
            storage,
            storage_core::AgentLogRecord {
                created_at_ms: None,
                level: storage_core::AgentLogLevel::Info,
                source: storage_core::AgentLogSource::Host,
                stage: "restart-fixture".into(),
                message: format!("diagnostic from cycle {cycle}"),
                details_json: Some(json!({"cycle":cycle})),
                context_json: None,
                origin: None,
                raw_error: None,
                traceback: None,
            },
        )
        .unwrap();
        assert_eq!(saved.sequence, cycle + 1, "logs use their own sequence");
        expected_logs.push(saved);
        assert_eq!(
            entries(&app, agent, StorageOperation::ReadHistoryEntries),
            history_before_log
        );
        assert_disk_schema_and_messages(&database_path, &message, expected_logs.len());
        assert_eq!(
            entries(&app, agent, StorageOperation::ReadAgentMessages),
            expected_messages
        );
        let merged = entries(&app, agent, StorageOperation::ReadHistoryEntries);
        assert_eq!(&merged[..previous_history.len()], previous_history);
        assert!(
            merged
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        let mut split = entries(&app, agent, StorageOperation::ReadAgentMessages);
        split.extend(entries(&app, agent, StorageOperation::ReadMclRecords));
        split.sort_by_key(|entry| entry.sequence);
        assert_eq!(split, merged);
        previous_history = merged;
        if cycle < 3 {
            let id = format!("restart-{cycle}");
            app.world().emit_event(RestartWorkspace {
                id: id.clone(),
                reply: request_reply(&(id.clone())),
                path: definition.clone(),
            });
            output = wait_workspace(&mut app, &id, true);
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), compose::shutdown_app(&mut app))
            .await
            .expect("application shutdown timed out")
            .unwrap();
    });
    assert!(
        app.world()
            .query_with::<workspace_core::Workspace>()
            .result()
            .is_empty()
    );
    assert!(
        app.world()
            .query_with::<agent_core::Agent>()
            .result()
            .is_empty()
    );
    assert!(
        app.world()
            .get_resource::<axum_plugin::AxumHandle>()
            .unwrap()
            .is_stopped()
    );
    assert_disk_schema_and_messages(&database_path, &message, expected_logs.len());
}

type OperationResult = Result<WorkspaceOperationOutput, workspace_core::WorkspaceError>;
thread_local! {static REPLIES:std::cell::RefCell<std::collections::BTreeMap<String,tokio::sync::oneshot::Receiver<OperationResult>>>=Default::default();}
fn request_reply(id: &str) -> workspace_core::WorkspaceOperationReply {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    REPLIES.with(|rows| rows.borrow_mut().insert(id.to_owned(), receiver));
    std::sync::Arc::new(std::sync::Mutex::new(Some(sender)))
}
fn poll_reply(id: &str) -> Option<OperationResult> {
    REPLIES.with(|rows| {
        rows.borrow_mut()
            .get_mut(id)
            .and_then(|receiver| receiver.try_recv().ok())
    })
}
