#![cfg(target_os = "linux")]
#[path = "internal/mailbox.rs"]
mod mailbox_tests;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::{App, Entity, World};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tool_core::{ToolError, ToolExecuteRequest, ToolExecuteResponse, ToolTasks};
use tool_plugin::ToolPlugin;

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(ToolPlugin::new().with_runner_path(env!("CARGO_BIN_EXE_tool_runner")));
    app
}
fn tool(root: &Path, name: &str, code: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tool.toml"),
        "schema_version=1\ndescription='Driver test'\n",
    )
    .unwrap();
    std::fs::write(dir.join("input.schema.json"), "{\"type\":\"object\"}").unwrap();
    std::fs::write(
        dir.join("main.lua"),
        format!("function configure() return {{timeout_ms=30000}} end\n{code}"),
    )
    .unwrap();
    dir.join("tool.toml")
}
fn execute(
    app: &mut App,
    entity: Entity,
    path: std::path::PathBuf,
    arguments: Value,
) -> oneshot::Receiver<Result<ToolExecuteResponse, ToolError>> {
    let (tx, rx) = oneshot::channel();
    app.world().emit_event(ToolExecuteRequest::Path {
        initiator: entity,
        path,
        arguments,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    rx
}
fn wait<T>(app: &mut App, mut rx: oneshot::Receiver<T>) -> T {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Ok(value) = rx.try_recv() {
            return value;
        }
        assert!(Instant::now() < until, "reply timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn drain(app: &mut App, entity: Entity) {
    let until = Instant::now() + Duration::from_secs(10);
    while !app
        .world()
        .get_component::<ToolTasks>(entity)
        .unwrap()
        .list()
        .is_empty()
    {
        app.tick();
        assert!(Instant::now() < until, "task did not drain");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn path_execution_needs_only_task_collection_and_validates_actual_schema() {
    let root = tempfile::tempdir().unwrap();
    let path = tool(
        root.path(),
        "tool",
        "function execute(arguments) margatroid.task:return_result(arguments.value) end",
    );
    let mut app = app();
    let entity = app.world_mut().spawn();
    app.world_mut().insert_component(entity, ToolTasks::new());
    let reply = execute(&mut app, entity, path.clone(), json!({"value":"direct"}));
    let result = wait(&mut app, reply).unwrap();
    assert_eq!(result.content, "direct");
    assert_eq!(result.alias, path.to_string_lossy());
    assert!(!result.call_id.is_empty());
    drain(&mut app, entity);
    std::fs::write(
        path.parent().unwrap().join("input.schema.json"),
        "{\"type\":\"object\",\"required\":[\"needed\"]}",
    )
    .unwrap();
    let reply = execute(&mut app, entity, path, json!({}));
    assert_eq!(
        wait(&mut app, reply).unwrap_err().kind,
        tool_core::ToolErrorKind::InvalidArguments
    );
    drain(&mut app, entity);
}

fn mount_workspace(app: &mut App, root: &Path) -> Entity {
    let managed = root.join("managed");
    for kind in ["normal", "temporary", "archived"] {
        std::fs::create_dir_all(managed.join("agents").join(kind)).unwrap();
    }
    let database = workspace_core::WorkspaceDatabase::create(
        managed.join("workspace.sqlite"),
        workspace_core::WorkspaceMetadata {
            address: "/demo".into(),
            alias: "demo".into(),
            project_root: root.into(),
        },
    )
    .unwrap();
    let entity = app.world_mut().spawn();
    app.world_mut()
        .insert_component(entity, address::Address::new("/demo").unwrap());
    app.world_mut().insert_component(
        entity,
        workspace_core::Workspace::new("demo".into(), root.into(), database),
    );
    app.world_mut().insert_component(entity, ToolTasks::new());
    entity
}
fn agent_fixture(world: &mut World) {
    let requests = world
        .event_reader::<agent_core::StartAgent<storage_core::AgentStorage>>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let Some(reply) = request.reply.lock().unwrap().take() else {
            continue;
        };
        assert!(request.persistence.is_open());
        let creator = world
            .get_component::<ToolTasks>(request.parent)
            .unwrap()
            .get(&request.task_id)
            .unwrap();
        let entity = world.spawn();
        let agent = agent_core::Agent::new(
            request.workspace,
            request.parent,
            request.task_id.clone(),
            request.project_root,
        );
        let ready = agent.ready_receiver();
        world.insert_component(entity, agent);
        world.insert_component(entity, address::Address::new(&request.address).unwrap());
        world.insert_component(entity, request.persistence.clone());
        let sender = Arc::new(world.closure_sender(RuntimePlugin::UPDATE));
        let storage = request.persistence;
        let handle = agent_core::AgentHandle::new(entity, ready, move || {
            let sender = sender.clone();
            let storage = storage.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || storage_core::close_storage(&storage))
                    .await
                    .unwrap()
                    .map_err(|e| {
                        agent_core::AgentError::new(
                            agent_core::AgentErrorKind::CleanupFailed,
                            e.to_string(),
                        )
                    })?;
                let (tx, rx) = oneshot::channel();
                sender.send(move |world| {
                    world.despawn(entity);
                    let _ = tx.send(());
                });
                rx.await.unwrap();
                Ok(())
            })
        });
        let child = handle.clone();
        creator
            .bind_child(
                entity,
                Arc::new(move || {
                    let child = child.clone();
                    Box::pin(async move {
                        child.close().await.map_err(|e| {
                            ToolError::new(tool_core::ToolErrorKind::CleanupFailed, e.to_string())
                        })
                    })
                }),
            )
            .unwrap();
        let _ = reply.send(Ok(handle));
    }
}
#[test]
fn driver_creation_registers_connected_storage_reuses_files_and_rejects_other_driver() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("default.toml"),
        "[custom]\nvalue='initial'\n",
    )
    .unwrap();
    let mut app = app();
    let workspace = mount_workspace(&mut app, root.path());
    app.add_system(RuntimePlugin::UPDATE, agent_fixture);
    let code = r#"function execute(arguments) local a=margatroid.agent:create({node_name=arguments.node_name,alias=arguments.alias,project_root=arguments.project_root,config_path=arguments.config_path,ui_path=arguments.ui_path});margatroid.task:return_result(a) end"#;
    let driver = tool(root.path(), "driver-a", code);
    let args = json!({"node_name":"coder","alias":"Coder","project_root":root.path(),"config_path":root.path().join("default.toml"),"ui_path":root.path().join("ui")});
    let reply = execute(&mut app, workspace, driver.clone(), args.clone());
    let info: Value = serde_json::from_str(&wait(&mut app, reply).unwrap().content).unwrap();
    assert_eq!(info["address"], "/demo/coder");
    drain(&mut app, workspace);
    let database = app
        .world()
        .get_component::<workspace_core::Workspace>(workspace)
        .unwrap()
        .database
        .clone();
    let record = database.agent_by_node("coder").unwrap();
    assert_eq!(record.tool_path, driver);
    let directory = database.agent_directory(&record).unwrap();
    std::fs::write(
        directory.join("setting.toml"),
        "[custom]\nvalue='preserved'\n",
    )
    .unwrap();
    let reply = execute(&mut app, workspace, driver, args.clone());
    wait(&mut app, reply).unwrap();
    drain(&mut app, workspace);
    assert!(std::fs::read_to_string(directory.join("setting.toml"))
        .unwrap()
        .contains("preserved"));
    let other = tool(root.path(), "driver-b", code);
    let reply = execute(&mut app, workspace, other, args);
    let failure = wait(&mut app, reply).unwrap_err();
    assert!(
        failure.message.contains("Driver") || failure.message.contains("different"),
        "{failure}"
    );
    drain(&mut app, workspace);
    assert_eq!(database.list_agents().unwrap().len(), 1);
}
