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
use workspace_core::{
    RestartWorkspace, RestartWorkspaceResult, StartWorkspace, StartWorkspaceResult, StopWorkspace,
    StopWorkspaceResult, WorkspaceOperationOutput,
};

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

fn wait_workspace(app: &mut App, id: &str, restart: bool) -> WorkspaceOperationOutput {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        let result = if restart {
            app.world()
                .event_reader::<RestartWorkspaceResult>()
                .into_iter()
                .find(|event| event.id == id)
                .map(|event| event.result.clone())
        } else {
            app.world()
                .event_reader::<StartWorkspaceResult>()
                .into_iter()
                .find(|event| event.id == id)
                .map(|event| event.result.clone())
        };
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
        id: format!("three-table:{text}"),
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

fn assert_disk_schema_and_messages(path: &Path, message: &Value) {
    let db = rusqlite::Connection::open(path).unwrap();
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(tables, ["agent_messages", "blocks", "mcl_records"]);
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
            "type",
            "content",
            "payload",
            "created_at_ms"
        ]
    );
    let message_type: String = db
        .query_row("SELECT type FROM agent_messages LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(message_type, message["type"].as_str().unwrap());
    assert_eq!(
        db.query_row("SELECT count(*) FROM main.blocks", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let payloads = db
        .prepare("SELECT payload FROM agent_messages ORDER BY sequence")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str::<Value>(&payload.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(payloads.as_slice(), std::slice::from_ref(message));
}

#[test]
fn workspace_restarts_keep_three_disk_tables_persistent_context_and_history_without_temp_row_growth()
 {
    let root = tempfile::tempdir().unwrap();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources"),
        root.path(),
    );
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let directory = root.path().join("workspaces/three-table");
    std::fs::create_dir_all(&directory).unwrap();
    let definition = directory.join("workspace.yaml");
    std::fs::write(&definition, format!(
        "name: three-table\nproject_root: {}\nmembers:\n  - name: coder\n    driver: driver:local/coder:latest\n    home_root: {}\n",
        project.display(), root.path().display())).unwrap();
    std::fs::write(root.path().join("config.toml"),
        "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:unmatched']\nbackend_state=['type:unmatched']\n").unwrap();
    std::fs::write(root.path().join("models.toml"),
        "[[models]]\nid='openrouter/deepseek-v4.1-flash'\nmodel='unused'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused-local-test'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n").unwrap();
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().to_owned(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    app.world().emit_event(StartWorkspace {
        id: "initial".into(),
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
    for cycle in 0..4 {
        let agent = output.members[0];
        let instance = app
            .world()
            .get_component::<DriverLuaVm>(agent)
            .unwrap()
            .instance
            .start_id
            .clone();
        assert!(instances.insert(instance));
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
        assert_disk_schema_and_messages(&database_path, &message);
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
                path: definition.clone(),
            });
            output = wait_workspace(&mut app, &id, true);
        }
    }
    app.world().emit_event(StopWorkspace {
        id: "final-stop".into(),
        workspace: output.workspace,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|event| event.id == "final-stop")
        {
            event.result.clone().unwrap();
            break;
        }
        assert!(Instant::now() < deadline, "workspace stop timed out");
        std::thread::yield_now();
    }
    assert_disk_schema_and_messages(&database_path, &message);
}
