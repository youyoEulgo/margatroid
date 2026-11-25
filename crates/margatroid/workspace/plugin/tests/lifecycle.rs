use address::{Address, WorldAddressExt};
use agent_core::Agent;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::{App, Entity, Resource, World};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};
use tool_core::{
    TaskFinish, ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse,
    ToolExecutionLimits, ToolTaskHandle, ToolTasks,
};
use workspace_core::*;
use workspace_plugin::WorkspacePlugin;

#[derive(Default)]
struct Captured(Vec<ToolExecuteRequest>);
impl Resource for Captured {}
fn capture_tools(world: &mut World) {
    let events = world
        .event_reader::<ToolExecuteRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    world
        .get_resource_mut::<Captured>()
        .unwrap()
        .0
        .extend(events);
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin);
    app.world_mut().insert_resource(Captured::default());
    app.add_plugin(WorkspacePlugin { root: root.into() })
        .add_system(RuntimePlugin::UPDATE, capture_tools);
    app
}
fn until(app: &mut App, mut condition: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition(app) {
        app.tick();
        std::thread::yield_now();
        assert!(Instant::now() < deadline, "Workspace test timed out");
    }
}
fn receive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let mut value = None;
    until(app, |_| match rx.try_recv() {
        Ok(v) => {
            value = Some(v);
            true
        }
        Err(oneshot::error::TryRecvError::Empty) => false,
        Err(e) => panic!("reply lost: {e}"),
    });
    value.unwrap()
}
fn reply<T>() -> (
    WorkspaceReply<T>,
    oneshot::Receiver<Result<T, WorkspaceError>>,
) {
    let (tx, rx) = oneshot::channel();
    (Arc::new(Mutex::new(Some(tx))), rx)
}
fn tool_reply() -> (
    WorkspaceAgentReply<ToolExecuteResponse, ToolError>,
    oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
) {
    let (tx, rx) = oneshot::channel();
    (Arc::new(Mutex::new(Some(tx))), rx)
}
fn disk_workspace(root: &Path, name: &str, project: &Path) -> WorkspaceDatabase {
    let directory = root.join(name);
    for category in ["normal", "temporary", "archived"] {
        fs::create_dir_all(directory.join("agents").join(category)).unwrap();
    }
    WorkspaceDatabase::create(
        directory.join("workspace.sqlite"),
        WorkspaceMetadata {
            address: format!("/{name}"),
            alias: name.into(),
            project_root: project.into(),
        },
    )
    .unwrap()
}
fn loaded(app: &mut App, node: &str) -> Entity {
    until(app, |app| app.world().entity_by_address(node).is_ok());
    app.world().entity_by_address(node).unwrap()
}
fn registered(database: &WorkspaceDatabase, node: &str, parent: &str) -> WorkspaceAgent {
    let operation = database.try_operation(node).unwrap();
    let record = database
        .prepare_agent(
            &operation,
            parent.into(),
            Some(node.into()),
            Some(node.into()),
            "/tools/driver/tool.toml".into(),
            json!({"node_name":node,"path":"src"}),
        )
        .unwrap();
    fs::create_dir(database.agent_directory(&record).unwrap()).unwrap();
    fs::write(
        database.agent_directory(&record).unwrap().join("state"),
        "preserve",
    )
    .unwrap();
    database.commit_agent(&operation, record).unwrap()
}
fn install_task(
    tasks: &ToolTasks,
    id: &str,
    gate: watch::Receiver<bool>,
    calls: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
) -> ToolTaskHandle {
    let finish: TaskFinish = Arc::new(move |handle| {
        calls.fetch_add(1, Ordering::SeqCst);
        let mut gate = gate.clone();
        let fail = fail.clone();
        Box::pin(async move {
            while !*gate.borrow_and_update() {
                if gate.changed().await.is_err() {
                    return Err(ToolError::new(
                        ToolErrorKind::CleanupFailed,
                        "test gate closed",
                    ));
                }
            }
            if fail.load(Ordering::SeqCst) {
                return Err(ToolError::new(
                    ToolErrorKind::CleanupFailed,
                    "injected cleanup failure",
                ));
            }
            handle.finish_children().await?;
            handle.retry_cleanups().await?;
            Ok(())
        })
    });
    let (tx, _rx) = oneshot::channel();
    let (handle, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        format!("call-{id}"),
        "driver".into(),
        tx,
        tasks.update_context(),
        None,
        finish,
    )
    .unwrap();
    tasks.register(id.into(), handle.clone()).unwrap();
    handle
}
fn bind_fake_agent(
    app: &mut App,
    workspace: Entity,
    parent: Entity,
    task: &ToolTaskHandle,
    id: &str,
    node: &str,
) -> Entity {
    let entity = app.world_mut().spawn();
    app.world_mut()
        .insert_component(entity, Address::new(node).unwrap());
    app.world_mut().insert_component(
        entity,
        Agent::new(workspace, parent, id.into(), "/project".into()),
    );
    let sender = app.world().closure_sender(RuntimePlugin::UPDATE);
    task.bind_child(
        entity,
        Arc::new(move || {
            let sender = sender.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                sender.send(move |world| {
                    world.despawn(entity);
                    let _ = tx.send(());
                });
                rx.await
                    .map_err(|e| ToolError::new(ToolErrorKind::CleanupFailed, e.to_string()))
            })
        }),
    )
    .unwrap();
    entity
}
#[test]
fn startup_loads_database_once_and_skips_invalid_entries() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    fs::create_dir(root.path().join("bad")).unwrap();
    fs::write(
        root.path().join("bad/workspace.yaml"),
        "old YAML is not a database",
    )
    .unwrap();
    let mut app = app(root.path());
    let entity = loaded(&mut app, "/demo");
    assert_eq!(
        app.world()
            .get_component::<Workspace>(entity)
            .unwrap()
            .state,
        WorkspaceState::Running
    );
    assert!(app
        .world()
        .get_component::<ToolTasks>(entity)
        .unwrap()
        .is_accepting());
    assert!(app.world().get_resource::<Captured>().unwrap().0.is_empty());
    let later = disk_workspace(root.path(), "later", project.path());
    later.close().unwrap();
    for _ in 0..50 {
        app.tick();
        std::thread::yield_now();
    }
    assert!(app.world().entity_by_address("/later").is_err());
    assert!(root.path().join("bad/workspace.yaml").exists());
}
#[test]
fn create_prepares_database_then_starts_entity_and_replies_original_once() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    let observed = Arc::new(Mutex::new(vec![]));
    let copy = observed.clone();
    app.world_mut()
        .insert_resource(WorkspaceUpdateContext::new(move |_, _, kind| {
            copy.lock().unwrap().push(kind)
        }));
    let (reply, mut rx) = reply();
    let event = CreateWorkspace {
        address: "/created".into(),
        alias: "Created".into(),
        project_root: project.path().into(),
        reply: reply.clone(),
    };
    app.world().emit_event(event.clone());
    app.world().emit_event(event);
    let entity = receive(&mut app, &mut rx).unwrap();
    assert!(reply.lock().unwrap().is_none());
    let workspace = app.world().get_component::<Workspace>(entity).unwrap();
    assert_eq!(workspace.database.metadata().unwrap().alias, "Created");
    assert!(workspace.database.is_open());
    assert_eq!(workspace.database.metadata().unwrap().address, "/created");
    assert_eq!(
        workspace.database.path(),
        root.path().join("created/workspace.sqlite")
    );
    assert!(workspace.database.list_agents().unwrap().is_empty());
    assert_eq!(
        *observed.lock().unwrap(),
        vec![WorkspaceUpdateKind::Installed]
    );
    assert_eq!(app.world().query_with::<Workspace>().result().len(), 1);
    for category in ["normal", "temporary", "archived"] {
        assert!(root.path().join("created/agents").join(category).is_dir());
    }
}
#[test]
fn duplicate_start_reuses_entity_without_closing_shared_connection() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let original = disk_workspace(root.path(), "demo", project.path());
    let mut app = app(root.path());
    let entity = loaded(&mut app, "/demo");
    let db = app
        .world()
        .get_component::<Workspace>(entity)
        .unwrap()
        .database
        .clone();
    for shared in [true, false] {
        let connection = if shared {
            db.clone()
        } else {
            WorkspaceDatabase::open(db.path()).unwrap()
        };
        let (reply, mut rx) = reply();
        app.world().emit_event(StartWorkspace {
            address: "/demo".into(),
            alias: "demo".into(),
            project_root: project.path().into(),
            database: connection.clone(),
            reply: Some(reply),
        });
        assert_eq!(receive(&mut app, &mut rx).unwrap(), entity);
        assert_eq!(connection.is_open(), shared);
        assert!(db.is_open());
    }
    let (reply, mut rx) = reply();
    app.world().emit_event(StartWorkspace {
        address: "/demo".into(),
        alias: "wrong".into(),
        project_root: project.path().into(),
        database: db.clone(),
        reply: Some(reply),
    });
    assert!(receive(&mut app, &mut rx).is_err());
    assert!(db.is_open());
    original.close().unwrap();
}
#[test]
fn new_and_existing_start_forward_the_exact_tool_reply_and_arguments() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let entity = loaded(&mut app, "/demo");
    let database = app
        .world()
        .get_component::<Workspace>(entity)
        .unwrap()
        .database
        .clone();
    let record = registered(&database, "coder", "/demo");
    for existing in [false, true] {
        let (sender, mut rx) = tool_reply();
        let args = if existing {
            json!({"node_name":"coder","alias":"coder","path":"src"})
        } else {
            json!({"node_name":"new-node","alias":"New","path":"custom"})
        };
        let request = if existing {
            StartWorkspaceAgent::Existing {
                workspace: entity,
                node_name: "coder".into(),
                reply: sender.clone(),
            }
        } else {
            StartWorkspaceAgent::New {
                workspace: entity,
                tool_path: "/tools/new/tool.toml".into(),
                arguments: args.clone(),
                reply: sender.clone(),
            }
        };
        app.world().emit_event(request);
        until(&mut app, |app| {
            !app.world().get_resource::<Captured>().unwrap().0.is_empty()
        });
        let request = app
            .world_mut()
            .get_resource_mut::<Captured>()
            .unwrap()
            .0
            .remove(0);
        match request {
            ToolExecuteRequest::Path {
                initiator,
                path,
                arguments,
                reply,
            } => {
                assert_eq!(initiator, entity);
                assert_eq!(arguments, args);
                assert_eq!(
                    path,
                    if existing {
                        record.tool_path.clone()
                    } else {
                        PathBuf::from("/tools/new/tool.toml")
                    }
                );
                assert!(Arc::ptr_eq(&reply, &sender));
                reply
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(Ok(ToolExecuteResponse {
                        call_id: "task".into(),
                        alias: "driver".into(),
                        content: "started".into(),
                        images: vec![],
                        failed: false,
                    }))
                    .unwrap();
            }
            _ => panic!("registered request used"),
        };
        assert_eq!(rx.try_recv().unwrap().unwrap().content, "started");
    }
    assert_eq!(database.list_agents().unwrap(), vec![record]);
    assert!(!root.path().join("demo/agents/normal/new-node").exists());
}
#[test]
fn new_rejects_existing_node_and_missing_parent_existing_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    registered(&db, "coder", "/demo");
    registered(&db, "child", "/demo/absent");
    db.close().unwrap();
    let mut app = app(root.path());
    let entity = loaded(&mut app, "/demo");
    let (sender, mut rx) = tool_reply();
    app.world().emit_event(StartWorkspaceAgent::New {
        workspace: entity,
        tool_path: "/tools/other".into(),
        arguments: json!({"node_name":"coder"}),
        reply: sender,
    });
    assert!(receive(&mut app, &mut rx).is_err());
    let (sender, mut rx) = tool_reply();
    app.world().emit_event(StartWorkspaceAgent::Existing {
        workspace: entity,
        node_name: "child".into(),
        reply: sender,
    });
    assert!(receive(&mut app, &mut rx).is_err());
    assert!(app.world().get_resource::<Captured>().unwrap().0.is_empty());
}
#[test]
fn existing_child_start_uses_recorded_parent_entity() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    registered(&db, "child", "/demo/parent");
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let parent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(parent, Address::new("/demo/parent").unwrap());
    app.world_mut().insert_component(
        parent,
        Agent::new(
            workspace,
            workspace,
            "parent-driver".into(),
            project.path().into(),
        ),
    );
    app.world_mut().insert_component(parent, ToolTasks::new());
    let (reply, _rx) = tool_reply();
    app.world().emit_event(StartWorkspaceAgent::Existing {
        workspace,
        node_name: "child".into(),
        reply,
    });
    until(&mut app, |app| {
        !app.world().get_resource::<Captured>().unwrap().0.is_empty()
    });
    match &app.world().get_resource::<Captured>().unwrap().0[0] {
        ToolExecuteRequest::Path { initiator, .. } => assert_eq!(*initiator, parent),
        _ => panic!("wrong request"),
    };
}
#[test]
fn archive_waits_actual_parent_task_then_moves_files_and_updates_registry() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    let record = registered(&db, "child", "/demo/parent");
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let db = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let parent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(parent, Address::new("/demo/parent").unwrap());
    app.world_mut().insert_component(
        parent,
        Agent::new(
            workspace,
            workspace,
            "parent-driver".into(),
            project.path().into(),
        ),
    );
    let tasks = ToolTasks::new();
    app.world_mut().insert_component(parent, tasks.clone());
    let (gate, wait) = watch::channel(false);
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(false));
    let task = install_task(&tasks, "correct-task", wait, calls.clone(), fail);
    let child = bind_fake_agent(
        &mut app,
        workspace,
        parent,
        &task,
        "correct-task",
        "/demo/parent/child",
    );
    let (ready, wait) = watch::channel(true);
    let wrong_calls = Arc::new(AtomicUsize::new(0));
    let own_tasks = ToolTasks::new();
    install_task(
        &own_tasks,
        "correct-task",
        wait,
        wrong_calls.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    app.world_mut().insert_component(child, own_tasks);
    let (sender, mut rx) = reply();
    app.world().emit_event(ArchiveAgent {
        workspace,
        node_name: "child".into(),
        reply: sender,
    });
    until(&mut app, |_| calls.load(Ordering::SeqCst) > 0);
    assert!(rx.try_recv().is_err());
    assert_eq!(
        db.agent_by_id(&record.id).unwrap().category,
        AgentCategory::Normal
    );
    assert!(app.world().is_alive(child));
    gate.send_replace(true);
    receive(&mut app, &mut rx).unwrap();
    assert!(!app.world().is_alive(child));
    assert_eq!(wrong_calls.load(Ordering::SeqCst), 0);
    let archived = db.agent_by_id(&record.id).unwrap();
    assert_eq!(archived.category, AgentCategory::Archived);
    assert_eq!(
        fs::read_to_string(db.agent_directory(&archived).unwrap().join("state")).unwrap(),
        "preserve"
    );
    drop(ready);
}
#[test]
fn archive_parent_mismatch_or_failed_cleanup_never_marks_archived() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    let record = registered(&db, "coder", "/demo");
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let db = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let tasks = app
        .world()
        .get_component::<ToolTasks>(workspace)
        .unwrap()
        .clone();
    let (_gate, wait) = watch::channel(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(true));
    let task = install_task(&tasks, "driver-task", wait, calls.clone(), fail.clone());
    let entity = bind_fake_agent(
        &mut app,
        workspace,
        workspace,
        &task,
        "driver-task",
        "/demo/coder",
    );
    let (sender, mut rx) = reply();
    app.world().emit_event(ArchiveAgent {
        workspace,
        node_name: "coder".into(),
        reply: sender,
    });
    assert!(receive(&mut app, &mut rx).is_err());
    assert_eq!(
        db.agent_by_id(&record.id).unwrap().category,
        AgentCategory::Normal
    );
    assert!(app.world().is_alive(entity));
    fail.store(false, Ordering::SeqCst);
    let (sender, mut rx) = reply();
    app.world().emit_event(ArchiveAgent {
        workspace,
        node_name: "coder".into(),
        reply: sender,
    });
    receive(&mut app, &mut rx).unwrap();
    assert_eq!(
        db.agent_by_id(&record.id).unwrap().category,
        AgentCategory::Archived
    );
}
#[test]
fn archive_nonrunning_record_does_not_require_agent_or_tasks() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    let record = registered(&db, "coder", "/demo");
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let db = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let (sender, mut rx) = reply();
    app.world().emit_event(ArchiveAgent {
        workspace,
        node_name: "coder".into(),
        reply: sender,
    });
    receive(&mut app, &mut rx).unwrap();
    assert_eq!(
        db.agent_by_id(&record.id).unwrap().category,
        AgentCategory::Archived
    );
}
#[test]
fn deletion_waits_real_task_cleanup_and_preserves_project_files() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("keep"), "project").unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let db = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let tasks = app
        .world()
        .get_component::<ToolTasks>(workspace)
        .unwrap()
        .clone();
    let (gate, wait) = watch::channel(false);
    let calls = Arc::new(AtomicUsize::new(0));
    install_task(
        &tasks,
        "driver",
        wait,
        calls.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let (sender, mut rx) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    until(&mut app, |_| calls.load(Ordering::SeqCst) > 0);
    assert!(rx.try_recv().is_err());
    assert!(db.is_open());
    assert!(!db.is_accepting());
    assert!(!tasks.is_accepting());
    assert!(root.path().join("demo").exists());
    let (sender, mut duplicate) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    assert_eq!(
        receive(&mut app, &mut duplicate).unwrap_err().kind,
        WorkspaceErrorKind::Busy
    );
    gate.send_replace(true);
    receive(&mut app, &mut rx).unwrap();
    assert!(!app.world().is_alive(workspace));
    assert!(!db.is_open());
    assert!(!root.path().join("demo").exists());
    assert_eq!(
        fs::read_to_string(project.path().join("keep")).unwrap(),
        "project"
    );
}
#[test]
fn deletion_failure_retains_entity_and_database_for_explicit_retry() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let tasks = app
        .world()
        .get_component::<ToolTasks>(workspace)
        .unwrap()
        .clone();
    let (_gate, wait) = watch::channel(true);
    let fail = Arc::new(AtomicBool::new(true));
    install_task(
        &tasks,
        "driver",
        wait,
        Arc::new(AtomicUsize::new(0)),
        fail.clone(),
    );
    let (sender, mut rx) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    assert_eq!(
        receive(&mut app, &mut rx).unwrap_err().kind,
        WorkspaceErrorKind::CloseFailed
    );
    assert_eq!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .state,
        WorkspaceState::Error
    );
    assert!(app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .is_open());
    assert!(root.path().join("demo").exists());
    fail.store(false, Ordering::SeqCst);
    let (sender, mut rx) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    receive(&mut app, &mut rx).unwrap();
    assert!(!app.world().is_alive(workspace));
}
#[test]
fn deletion_refuses_project_containment_before_closing_or_destroying() {
    for project_inside in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let db = disk_workspace(root.path(), "demo", project.path());
        db.close().unwrap();
        let mut app = app(root.path());
        let workspace = loaded(&mut app, "/demo");
        let bad = if project_inside {
            let p = root.path().join("demo/project");
            fs::create_dir(&p).unwrap();
            p
        } else {
            root.path().to_owned()
        };
        app.world_mut()
            .get_component_mut::<Workspace>(workspace)
            .unwrap()
            .project_root = bad;
        let (sender, mut rx) = reply();
        app.world().emit_event(DeleteWorkspace {
            workspace,
            reply: sender,
        });
        assert_eq!(
            receive(&mut app, &mut rx).unwrap_err().kind,
            WorkspaceErrorKind::InvalidRequest
        );
        let ws = app.world().get_component::<Workspace>(workspace).unwrap();
        assert_eq!(ws.state, WorkspaceState::Running);
        assert!(ws.database.is_open());
        assert!(ws.database.is_accepting());
        assert!(root.path().join("demo/workspace.sqlite").exists());
    }
}
#[test]
fn create_refuses_project_overlap_or_existing_directory() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    for project in [root.path().to_owned(), {
        let p = root.path().join("nested/project");
        fs::create_dir_all(&p).unwrap();
        p
    }] {
        let (sender, mut rx) = reply();
        app.world().emit_event(CreateWorkspace {
            address: "/nested".into(),
            alias: "Nested".into(),
            project_root: project,
            reply: sender,
        });
        assert!(receive(&mut app, &mut rx).is_err());
    }
    assert!(app.world().query_with::<Workspace>().result().is_empty());
}

#[test]
fn archive_rejects_agent_whose_parent_disagrees_with_registry() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    let record = registered(&db, "coder", "/demo");
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let database = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let parent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(parent, Address::new("/demo/other").unwrap());
    app.world_mut().insert_component(
        parent,
        Agent::new(
            workspace,
            workspace,
            "parent-driver".into(),
            project.path().into(),
        ),
    );
    let tasks = ToolTasks::new();
    app.world_mut().insert_component(parent, tasks.clone());
    let (_gate, wait) = watch::channel(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let task = install_task(
        &tasks,
        "driver",
        wait,
        calls.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let child = bind_fake_agent(&mut app, workspace, parent, &task, "driver", "/demo/coder");
    let (sender, mut rx) = reply();
    app.world().emit_event(ArchiveAgent {
        workspace,
        node_name: "coder".into(),
        reply: sender,
    });
    assert_eq!(
        receive(&mut app, &mut rx).unwrap_err().kind,
        WorkspaceErrorKind::ParentMismatch
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(app.world().is_alive(child));
    assert_eq!(
        database.agent_by_id(&record.id).unwrap().category,
        AgentCategory::Normal
    );
}

#[test]
fn deletion_waits_admitted_creation_operation_even_when_task_set_is_empty() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let database = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let operation = database.try_operation("in-flight").unwrap();
    let (sender, mut rx) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    until(&mut app, |app| {
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .state
            == WorkspaceState::Deleting
    });
    for _ in 0..20 {
        app.tick();
        std::thread::yield_now();
    }
    assert!(rx.try_recv().is_err());
    assert!(database.is_open());
    assert!(root.path().join("demo").is_dir());
    drop(operation);
    receive(&mut app, &mut rx).unwrap();
    assert!(!database.is_open());
    assert!(!root.path().join("demo").exists());
}

#[cfg(unix)]
#[test]
fn directory_replacement_during_cleanup_is_rejected_before_database_close() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), "external").unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let workspace = loaded(&mut app, "/demo");
    let database = app
        .world()
        .get_component::<Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let tasks = app
        .world()
        .get_component::<ToolTasks>(workspace)
        .unwrap()
        .clone();
    let (gate, wait) = watch::channel(false);
    let calls = Arc::new(AtomicUsize::new(0));
    install_task(
        &tasks,
        "driver",
        wait,
        calls.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let (sender, mut rx) = reply();
    app.world().emit_event(DeleteWorkspace {
        workspace,
        reply: sender,
    });
    until(&mut app, |_| calls.load(Ordering::SeqCst) > 0);
    fs::rename(root.path().join("demo"), root.path().join("held")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("demo")).unwrap();
    gate.send_replace(true);
    let error = receive(&mut app, &mut rx).unwrap_err();
    assert_eq!(error.kind, WorkspaceErrorKind::DeleteFailed);
    assert!(!error.entity_removed);
    assert!(app.world().is_alive(workspace));
    assert!(database.is_open());
    assert_eq!(
        fs::read_to_string(outside.path().join("keep")).unwrap(),
        "external"
    );
    assert!(root.path().join("held/workspace.sqlite").exists());
}

#[test]
fn work_completion_marker_is_consumed_after_workspace_start_installation() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    db.close().unwrap();
    let mut app = app(root.path());
    let pending = app
        .world()
        .get_resource::<WorkspacePendingWork>()
        .unwrap()
        .clone();
    assert!(!pending.is_idle());
    until(&mut app, |_| pending.is_idle());
    assert!(pending.load_completed());
    assert!(app.world().entity_by_address("/demo").is_ok());
    let (sender, mut receiver) = reply();
    app.world().emit_event(CreateWorkspace {
        address: "/created".into(),
        alias: "Created".into(),
        project_root: project.path().into(),
        reply: sender,
    });
    app.tick();
    assert!(!pending.is_idle());
    let entity = receive(&mut app, &mut receiver).unwrap();
    until(&mut app, |_| pending.is_idle());
    assert_eq!(app.world().entity_by_address("/created").unwrap(), entity);
}

#[test]
fn existing_start_rebuilds_identity_from_record_and_retains_driver_parameters() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let db = disk_workspace(root.path(), "demo", project.path());
    let op = db.try_operation("coder").unwrap();
    let original_arguments = json!({"node_name":"stale-node","alias":"Old alias","path":"src","driver_setting":{"mode":"review"}});
    let record = db
        .prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            Some("Old alias".into()),
            "/tools/driver/tool.toml".into(),
            original_arguments.clone(),
        )
        .unwrap();
    fs::create_dir(db.agent_directory(&record).unwrap()).unwrap();
    db.commit_agent(&op, record.clone()).unwrap();
    drop(op);
    db.close().unwrap();
    let mut app = app(root.path());
    let entity = loaded(&mut app, "/demo");
    let database = app
        .world()
        .get_component::<Workspace>(entity)
        .unwrap()
        .database
        .clone();
    for alias in [Some("Current alias".to_owned()), None] {
        database
            .rename_agent_alias(&record.id, alias.clone())
            .unwrap();
        let (sender, _rx) = tool_reply();
        app.world().emit_event(StartWorkspaceAgent::Existing {
            workspace: entity,
            node_name: "coder".into(),
            reply: sender,
        });
        until(&mut app, |app| {
            !app.world().get_resource::<Captured>().unwrap().0.is_empty()
        });
        let request = app
            .world_mut()
            .get_resource_mut::<Captured>()
            .unwrap()
            .0
            .remove(0);
        let ToolExecuteRequest::Path {
            initiator,
            path,
            arguments,
            ..
        } = request
        else {
            panic!("expected path execution")
        };
        assert_eq!(initiator, entity);
        assert_eq!(path, record.tool_path);
        let mut expected = original_arguments.clone();
        expected["node_name"] = json!("coder");
        match alias {
            Some(value) => expected["alias"] = json!(value),
            None => {
                expected.as_object_mut().unwrap().remove("alias");
            }
        }
        assert_eq!(arguments, expected);
        assert_eq!(
            database.agent_by_id(&record.id).unwrap().tool_arguments,
            original_arguments,
            "dispatch is not itself a successful Driver startup"
        );
    }
}
