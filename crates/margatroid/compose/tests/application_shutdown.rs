use agent_core::{Agent, AgentState};
use compose::{ApplicationOptions, build_app};
use core_plugin::App;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{StartWorkspace, StopWorkspace, Workspace, WorkspaceState};
fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
async fn drive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Ok(value) = rx.try_recv() {
            return value;
        }
        assert!(Instant::now() < deadline, "startup did not settle");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
async fn shutdown_phase(phase: WorkspaceState) {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
    write(
        root.path(),
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\ncontext_window='128k'\n",
    );
    for name in ["tools", "prompts", "sandboxes", "skills"] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    let tail = match phase {
        WorkspaceState::Starting => "margatroid.task:receive()",
        WorkspaceState::Error => "error('fixture initialization failed')",
        _ => {
            "local result=margatroid.mcl:execute(agent.address,'EFFECT ready');assert(not result.Error);margatroid:wait(agent.address)"
        }
    };
    write(
        root.path(),
        "agent-drivers/probe/shutdown/dev/base.lua",
        &format!(
            "function configure()return {{timeout_ms=0}}end\nfunction execute(_,context)\nlocal agent=margatroid.agent:create({{name=context.name,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'}})\n{tail}\nend"
        ),
    );
    write(
        root.path(),
        "workspaces/margatroid/probe/dev/workspace.yaml",
        &format!(
            "project_root: {}\nmembers:\n- name: target\n  driver: driver:probe/shutdown:dev\n  home_root: {}\n",
            root.path().display(),
            root.path().display()
        ),
    );
    write(
        root.path(),
        "agent-drivers/probe/shutdown/dev/setting.toml",
        "",
    );
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    let (tx, mut start) = oneshot::channel();
    app.world().emit_event(StartWorkspace {
        id: "start".into(),
        path: root
            .path()
            .join("workspaces/margatroid/probe/dev/workspace.yaml"),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let workspace = if phase == WorkspaceState::Starting {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.tick();
            let mounted = app
                .world()
                .query_with::<Agent>()
                .result()
                .into_iter()
                .find(|entity| {
                    app.world()
                        .get_component::<agent_core::AgentSession>(*entity)
                        .is_some()
                });
            if let Some(agent) = mounted {
                break app
                    .world()
                    .get_component::<Agent>(agent)
                    .unwrap()
                    .workspace();
            }
            assert!(Instant::now() < deadline, "Agent never mounted");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    } else {
        let started = drive(&mut app, &mut start).await;
        if phase == WorkspaceState::Error {
            assert!(started.is_err());
            app.world().query_with::<Workspace>().result()[0]
        } else {
            started.unwrap().workspace
        }
    };
    let close = app
        .world()
        .get_component::<workspace_core::WorkspaceExecutor>(workspace)
        .unwrap()
        .close
        .clone();
    let cleaned = Arc::new(AtomicBool::new(false));
    let mut stop_reply = None;
    let mut release_task = None;
    if phase == WorkspaceState::Stopping {
        let agent = app.world().query_with::<Agent>().result()[0];
        assert_eq!(
            app.world().get_component::<Agent>(agent).unwrap().state(),
            AgentState::Running
        );
        let cancel = CancellationToken::new();
        let observed = cancel.clone();
        let (canceled, wait_cancel) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        let complete = cleaned.clone();
        let handle = inference_core::InferenceHandle {
            request_id: "held-cleanup".into(),
            turn: Some("held-turn".into()),
            cancel,
            stop_stream: Arc::new(|| {}),
            task: Arc::new(inference_core::InferenceTaskState::default()),
        };
        handle
            .attach_task(tokio::spawn(async move {
                observed.cancelled().await;
                let _ = canceled.send(());
                waiting.await.unwrap();
                complete.store(true, Ordering::Release);
            }))
            .unwrap();
        app.world_mut()
            .get_component_mut::<inference_core::ActiveInference>(agent)
            .unwrap()
            .current = Some(handle);
        let (tx, rx) = oneshot::channel();
        app.world().emit_event(StopWorkspace {
            id: "existing-stop".into(),
            workspace,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        stop_reply = Some(rx);
        app.tick();
        release_task = Some(tokio::spawn(async move {
            wait_cancel.await.unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            release.send(()).unwrap();
        }));
    }
    assert_eq!(
        app.world()
            .get_component::<Workspace>(workspace)
            .unwrap()
            .state,
        phase
    );
    tokio::time::timeout(Duration::from_secs(5), compose::shutdown_app(&mut app))
        .await
        .expect("application shutdown ignored a non-running Workspace")
        .unwrap();
    if let Some(task) = release_task {
        task.await.unwrap();
        assert!(cleaned.load(Ordering::Acquire));
    }
    if let Some(mut reply) = stop_reply {
        assert!(
            reply.try_recv().unwrap().is_ok(),
            "existing stop lost its original result"
        );
    }
    assert!(close.is_joined());
    assert!(app.world().query_with::<Workspace>().result().is_empty());
    assert!(app.world().query_with::<Agent>().result().is_empty());
    assert!(
        app.world()
            .query_with::<lua_runtime_core::DriverLuaVm>()
            .result()
            .is_empty()
    );
}
#[tokio::test]
async fn application_shutdown_cancels_a_starting_workspace_before_startup_timeout() {
    shutdown_phase(WorkspaceState::Starting).await;
}
#[tokio::test]
async fn application_shutdown_cleans_a_running_workspace() {
    shutdown_phase(WorkspaceState::Running).await;
}
#[tokio::test]
async fn application_shutdown_cleans_an_error_workspace_without_a_prior_stop_request() {
    shutdown_phase(WorkspaceState::Error).await;
}
#[tokio::test]
async fn application_shutdown_waits_for_existing_stopping_cleanup_and_original_reply() {
    shutdown_phase(WorkspaceState::Stopping).await;
}
