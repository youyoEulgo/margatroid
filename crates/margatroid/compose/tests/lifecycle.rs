#![allow(dead_code, unused_imports)]
use address::{Address, WorldAddressExt};
use agent_core::{Agent, AgentHandle, AgentState, StartAgent, StopAgent, WorldAgentExt};
use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity};
use mcl_core::{MclCommand, MclInstruction, MclValue};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_core::AgentStorage;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_core::{
    TaskFinish, ToolError, ToolErrorKind, ToolExecuteResponse, ToolExecutionLimits, ToolTaskHandle,
    ToolTasks,
};
use workspace_core::{
    CreateWorkspace, Workspace, WorkspaceAgent, WorkspaceDatabase, WorkspaceState,
};

fn runner() -> PathBuf {
    let path = std::env::var_os("MARGATROID_TEST_RUNNER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/margatroid")
        });
    assert!(
        path.is_file(),
        "build the actual runner with cargo build -p margatroid, or set MARGATROID_TEST_RUNNER: {}",
        path.display()
    );
    path.canonicalize().unwrap()
}
fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn prepare_root(root: &Path) {
    write(root, "config.toml", "[server]\nbind='127.0.0.1:0'\n");
    write(
        root,
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='test'\ncontext_window='128k'\n",
    );
    for name in [
        "tools",
        "prompts",
        "sandboxes",
        "skills",
        "workspaces",
        "project",
    ] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
}
async fn drive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        match rx.try_recv() {
            Ok(value) => return value,
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(error) => panic!("reply lost: {error}"),
        }
        assert!(Instant::now() < deadline, "lifecycle reply timed out");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
async fn tick_until(app: &mut App, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(app) {
        app.tick();
        assert!(Instant::now() < deadline, "lifecycle state timed out");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
async fn fixture() -> (tempfile::TempDir, App, Entity) {
    let root = tempfile::tempdir().unwrap();
    prepare_root(root.path());
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: runner(),
    })
    .unwrap();
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(CreateWorkspace {
        address: "/demo".into(),
        alias: "Demo".into(),
        project_root: root.path().join("project"),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let workspace = drive(&mut app, &mut rx).await.unwrap();
    (root, app, workspace)
}
fn create_task(app: &App, parent: Entity, id: &str) -> ToolTaskHandle {
    let tasks = app
        .world()
        .get_component::<ToolTasks>(parent)
        .unwrap()
        .clone();
    let owner = tasks.clone();
    let task_id = id.to_owned();
    let runtime = tokio::runtime::Handle::current();
    let running = Arc::new(tokio::sync::Mutex::new(()));
    let finish: TaskFinish = Arc::new(move |handle| {
        let runtime = runtime.clone();
        let owner = owner.clone();
        let task_id = task_id.clone();
        let running = running.clone();
        let (tx, rx) = oneshot::channel();
        runtime.spawn(async move {
            let _run = running.lock().await;
            let result = if handle.is_cleanup_complete() {
                Ok(())
            } else {
                async {
                    handle.finish_children().await?;
                    handle.retry_cleanups().await?;
                    if !owner.complete_cleanup(&task_id, &handle, None) {
                        return Err(ToolError::new(
                            ToolErrorKind::CleanupFailed,
                            "cleanup did not retire task",
                        ));
                    }
                    Ok(())
                }
                .await
            };
            let _ = tx.send(result);
        });
        Box::pin(async move {
            rx.await
                .map_err(|e| ToolError::new(ToolErrorKind::CleanupFailed, e.to_string()))?
        })
    });
    let (tx, _rx) = oneshot::channel();
    let (handle, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        id.into(),
        "test-driver".into(),
        tx,
        tasks.update_context(),
        None,
        finish,
    )
    .unwrap();
    tasks.register(id.into(), handle.clone()).unwrap();
    handle
}
async fn mount(
    app: &mut App,
    workspace: Entity,
    node: &str,
    task_id: &str,
) -> (AgentHandle, WorkspaceAgent) {
    let database = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let operation = database.try_operation(node).unwrap();
    let candidate = database
        .prepare_agent(
            &operation,
            "/demo".into(),
            Some(node.into()),
            Some("Agent".into()),
            "/test/driver/tool.toml".into(),
            json!({"node_name":node}),
        )
        .unwrap();
    let directory = database.agent_directory(&candidate).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    let storage = storage_core::open_storage(
        &directory.join("state.sqlite"),
        &directory.join("setting.toml"),
        "",
    )
    .unwrap();
    let record = database.commit_agent(&operation, candidate).unwrap();
    create_task(app, workspace, task_id);
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StartAgent {
        workspace,
        parent: workspace,
        task_id: task_id.into(),
        address: format!("/demo/{node}"),
        project_root: app
            .world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .project_root
            .clone(),
        ui_path: directory.join("ui"),
        persistence: storage,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let handle = drive(app, &mut rx).await.unwrap();
    drop(operation);
    (handle, record)
}
async fn stop(app: &mut App, address: &str) {
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StopAgent {
        address: address.into(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(app, &mut rx).await.unwrap();
}
async fn command(
    app: &mut App,
    source: Entity,
    target: Entity,
    text: &str,
    arguments: Vec<Value>,
) -> MclValue {
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("command:{text}"),
        source,
        source_address: app
            .world()
            .get_component::<Address>(source)
            .unwrap()
            .as_str()
            .into(),
        source_instance: None,
        target,
        instruction: MclInstruction {
            text: text.into(),
            arguments,
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(app, &mut rx).await.unwrap()
}
async fn shutdown(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(10), compose::shutdown_app(app))
        .await
        .expect("application shutdown timed out")
        .unwrap();
}

const DRIVER: &str = r#"
function configure() return {timeout_ms=0} end
function execute(args,context)
 local agent=margatroid.agent:create({node_name=args.node_name,alias=args.alias,project_root=context.project_root,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 local ready=margatroid.mcl:execute(agent.address,'EFFECT ready')
 if ready.Error then error(ready.Error.message) end
 margatroid.task:return_result(agent.address)
 margatroid.task:receive()
end
"#;
fn driver(root: &Path) -> PathBuf {
    let dir = root.join("tools/probe/driver/dev");
    write(
        &dir,
        "tool.toml",
        "schema_version=1\nname='driver'\ndescription='Create a persistent test Agent'\n",
    );
    write(
        &dir,
        "input.schema.json",
        r#"{"type":"object","properties":{"node_name":{"type":"string"},"alias":{"type":"string"}},"required":["node_name"],"additionalProperties":false}"#,
    );
    write(&dir, "main.lua", DRIVER);
    write(&dir, "setting.toml", "");
    write(&dir, "ui/index.html", "<html>Agent</html>");
    dir.join("tool.toml")
}
async fn launch(app: &mut App, workspace: Entity, path: Option<PathBuf>) -> ToolExecuteResponse {
    let (tx, mut rx) = oneshot::channel::<Result<ToolExecuteResponse, ToolError>>();
    let reply = Arc::new(Mutex::new(Some(tx)));
    let request = if let Some(tool_path) = path {
        workspace_core::StartWorkspaceAgent::New {
            workspace,
            tool_path,
            arguments: json!({"node_name":"coder","alias":"Coder"}),
            reply,
        }
    } else {
        workspace_core::StartWorkspaceAgent::Existing {
            workspace,
            node_name: "coder".into(),
            reply,
        }
    };
    app.world().emit_event(request);
    drive(app, &mut rx).await.unwrap()
}
#[tokio::test]
async fn driver_tool_auto_registers_stops_restarts_and_archives_actual_agent() {
    let (root, mut app, workspace) = fixture().await;
    let path = driver(root.path());
    let result = launch(&mut app, workspace, Some(path.clone())).await;
    assert!(!result.failed, "{}", result.content);
    assert_eq!(result.content, "/demo/coder");
    let first = app.world().entity_by_address("/demo/coder").unwrap();
    let database = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let record = database.agent_by_node("coder").unwrap();
    assert_eq!(record.tool_path, path.canonicalize().unwrap());
    assert_eq!(
        record.tool_arguments,
        json!({"node_name":"coder","alias":"Coder"})
    );
    assert_eq!(
        app.world().get_component::<Agent>(first).unwrap().state(),
        AgentState::Running
    );
    stop(&mut app, "/demo/coder").await;
    assert!(!app.world().is_alive(first));
    assert!(
        database
            .agent_directory(&record)
            .unwrap()
            .join("state.sqlite")
            .is_file()
    );
    let result = launch(&mut app, workspace, None).await;
    assert!(!result.failed, "{}", result.content);
    let second = app.world().entity_by_address("/demo/coder").unwrap();
    assert_ne!(first, second);
    assert_eq!(database.agent_by_node("coder").unwrap().id, record.id);
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(workspace_core::ArchiveAgent {
        workspace,
        node_name: "coder".into(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(&mut app, &mut rx).await.unwrap();
    assert!(!app.world().is_alive(second));
    let archived = database.agent_by_id(&record.id).unwrap();
    assert_eq!(archived.category, workspace_core::AgentCategory::Archived);
    assert!(
        database
            .agent_directory(&archived)
            .unwrap()
            .join("state.sqlite")
            .is_file()
    );
    shutdown(&mut app).await;
}
