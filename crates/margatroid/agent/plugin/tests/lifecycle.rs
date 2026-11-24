use address::Address;
use agent_core::*;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use storage_core::{open_storage, AgentStorage};
use tokio::sync::oneshot;
use tool_core::*;
use workspace_core::{Workspace, WorkspaceDatabase, WorkspaceMetadata};

struct Fixture {
    app: App,
    root: tempfile::TempDir,
    workspace: Entity,
    task: ToolTaskHandle,
    storage: AgentStorage,
}
impl Fixture {
    fn new(sync: AgentUpdateContext) -> Self {
        let root = tempfile::tempdir().unwrap();
        for kind in ["normal", "temporary", "archived"] {
            std::fs::create_dir_all(root.path().join("workspace/agents").join(kind)).unwrap();
        }
        let database = WorkspaceDatabase::create(
            root.path().join("workspace/workspace.sqlite"),
            WorkspaceMetadata {
                address: "/demo".into(),
                alias: "Demo".into(),
                project_root: root.path().into(),
            },
        )
        .unwrap();
        let storage_root = root.path().join("workspace/agents/normal/coder");
        std::fs::create_dir_all(&storage_root).unwrap();
        let storage = open_storage(
            &storage_root.join("state.sqlite"),
            &storage_root.join("setting.toml"),
            "",
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AgentPlugin::new(sync));
        let workspace = app.world_mut().spawn();
        app.world_mut()
            .insert_component(workspace, Address::new("/demo").unwrap());
        app.world_mut().insert_component(
            workspace,
            Workspace::new("Demo".into(), root.path().into(), database),
        );
        let tasks = ToolTasks::new();
        let (tx, _) = oneshot::channel();
        let captured = tasks.clone();
        let finish: TaskFinish = Arc::new(move |handle| {
            let tasks = captured.clone();
            Box::pin(async move {
                handle.cancel();
                handle.finish_children().await?;
                handle.retry_cleanups().await?;
                if !tasks.complete_cleanup("driver", &handle, None) {
                    return Err(ToolError::new(
                        ToolErrorKind::CleanupFailed,
                        "cleanup incomplete",
                    ));
                }
                Ok(())
            })
        });
        let (task, _) = ToolTaskHandle::new(
            &ToolExecutionLimits::default(),
            "call-driver".into(),
            "driver".into(),
            tx,
            tasks.update_context(),
            None,
            finish,
        )
        .unwrap();
        tasks.register("driver".into(), task.clone()).unwrap();
        app.world_mut().insert_component(workspace, tasks);
        Self {
            app,
            root,
            workspace,
            task,
            storage,
        }
    }
    fn request(
        &self,
    ) -> (
        StartAgent<AgentStorage>,
        oneshot::Receiver<Result<AgentHandle, AgentError>>,
    ) {
        let (tx, rx) = oneshot::channel();
        (
            StartAgent {
                workspace: self.workspace,
                parent: self.workspace,
                task_id: "driver".into(),
                address: "/demo/coder".into(),
                project_root: self.root.path().into(),
                ui_path: self.root.path().join("ui"),
                persistence: self.storage.clone(),
                reply: Arc::new(Mutex::new(Some(tx))),
            },
            rx,
        )
    }
    fn tick(&mut self) {
        self.app.tick();
    }
    fn until<T>(&mut self, rx: &mut oneshot::Receiver<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.tick();
            match rx.try_recv() {
                Ok(v) => return v,
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(e) => panic!("reply lost: {e}"),
            };
            assert!(Instant::now() < deadline, "reply timed out");
            std::thread::yield_now();
        }
    }
    fn close(&mut self, handle: AgentHandle) -> Result<(), AgentError> {
        let (tx, mut rx) = oneshot::channel();
        let wait = handle.close();
        let rt = self
            .app
            .world()
            .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
            .unwrap();
        rt.spawn(async move {
            let _ = tx.send(wait.await);
        });
        self.until(&mut rx)
    }
}
fn quiet() -> AgentUpdateContext {
    AgentUpdateContext::new(|_, _, _| vec![], |_, _| {})
}

#[test]
fn creates_all_components_synchronously_and_closes_storage_without_finishing_creator() {
    let mut f = Fixture::new(quiet());
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    f.tick();
    let handle = rx
        .try_recv()
        .expect("creation must reply in its system frame")
        .unwrap();
    let entity = handle.entity();
    let world = f.app.world();
    let agent = world.get_component::<Agent>(entity).unwrap();
    assert_eq!(agent.task_owner(), (f.workspace, "driver"));
    assert_eq!(agent.state(), AgentState::Starting);
    assert!(world.contains_component::<Address>(entity));
    assert!(world.contains_component::<mcl_core::MclQueue>(entity));
    assert!(world.contains_component::<tool_resource_core::Tools>(entity));
    assert!(world.contains_component::<ToolTasks>(entity));
    assert!(world.contains_component::<inference_core::ActiveInference>(entity));
    assert!(world.contains_component::<AgentStorage>(entity));
    f.close(handle).unwrap();
    assert!(!f.app.world().is_alive(entity));
    assert!(!f.storage.is_open());
    assert!(
        !f.task.cancel_token().is_cancelled(),
        "Agent close cannot close creator task"
    );
}

#[test]
fn stop_finds_parent_task_and_waits_for_bound_agent() {
    let mut f = Fixture::new(quiet());
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    let handle = f.until(&mut rx).unwrap();
    let (tx, mut rx) = oneshot::channel();
    f.app.world().send_event(StopAgent {
        address: "/demo/coder".into(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    f.until(&mut rx).unwrap();
    assert!(!f.app.world().is_alive(handle.entity()));
    assert!(f.task.is_cleanup_complete());
    assert!(!f.storage.is_open());
}

#[test]
fn source_shutdown_is_awaited_and_close_survives_dropped_waiter() {
    let released = Arc::new(AtomicBool::new(false));
    let flag = released.clone();
    let sync = AgentUpdateContext::new(
        move |_, _, kind| {
            if matches!(kind, AgentUpdateKind::SourcesClosing) {
                let flag = flag.clone();
                vec![Box::pin(async move {
                    while !flag.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                    Ok(())
                })]
            } else {
                vec![]
            }
        },
        |_, _| {},
    );
    let mut f = Fixture::new(sync);
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    let handle = f.until(&mut rx).unwrap();
    drop(handle.close());
    for _ in 0..100 {
        f.tick();
        std::thread::yield_now();
    }
    assert!(f.app.world().is_alive(handle.entity()));
    assert!(f.storage.is_open());
    released.store(true, Ordering::Release);
    f.close(handle.clone()).unwrap();
    assert!(!f.app.world().is_alive(handle.entity()));
}

#[test]
fn failed_close_can_retry_without_reopening_admission() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let sync = AgentUpdateContext::new(
        move |_, _, kind| {
            if matches!(kind, AgentUpdateKind::SourcesClosing)
                && observed.fetch_add(1, Ordering::AcqRel) == 0
            {
                vec![Box::pin(async {
                    Err(AgentError::new(
                        AgentErrorKind::CleanupFailed,
                        "injected monitor failure",
                    ))
                })]
            } else {
                vec![]
            }
        },
        |_, _| {},
    );
    let mut f = Fixture::new(sync);
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    let handle = f.until(&mut rx).unwrap();
    assert!(f.close(handle.clone()).is_err());
    let agent = f
        .app
        .world()
        .get_component::<Agent>(handle.entity())
        .unwrap();
    assert_eq!(agent.state(), AgentState::Closing);
    assert!(!f
        .app
        .world()
        .get_component::<ToolTasks>(handle.entity())
        .unwrap()
        .is_accepting());
    f.close(handle.clone()).unwrap();
    assert!(!f.app.world().is_alive(handle.entity()));
    assert!(attempts.load(Ordering::Acquire) >= 2);
}

#[test]
fn dropped_creation_receiver_rolls_back_entity_and_connection() {
    let mut f = Fixture::new(quiet());
    let (req, rx) = f.request();
    drop(rx);
    f.app.world().send_event(req);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        f.tick();
        if !f.storage.is_open() && f.app.world().query_with::<Agent>().result().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
}

#[test]
fn rejects_missing_task_and_duplicate_address() {
    let mut f = Fixture::new(quiet());
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    let handle = f.until(&mut rx).unwrap();
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    assert!(f.until(&mut rx).is_err());
    let (req, mut rx) = f.request();
    let mut req = req;
    req.parent = handle.entity();
    req.address = "/demo/coder/child".into();
    req.task_id = "missing".into();
    f.app.world().send_event(req);
    assert!(f.until(&mut rx).is_err());
    f.close(handle).unwrap();
}

#[test]
fn close_waits_for_mcl_and_actual_inference_exit() {
    use mcl_core::WorldMclExt;
    let mut f = Fixture::new(quiet());
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    let handle = f.until(&mut rx).unwrap();
    let entity = handle.entity();
    let drain = f.app.world().mcl_drain(entity).unwrap();
    drain.started();
    let released = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    let inference = inference_core::InferenceHandle {
        request_id: "inference".into(),
        turn: None,
        cancel: Default::default(),
        source_alive: Arc::new(AtomicBool::new(true)),
        stop_stream: Arc::new(|| {}),
        task: Default::default(),
    };
    f.app
        .world_mut()
        .get_component_mut::<inference_core::ActiveInference>(entity)
        .unwrap()
        .current = Some(inference.clone());
    let (tx, mut installed) = oneshot::channel();
    let worker = inference.clone();
    let flag = released.clone();
    let ended = stopped.clone();
    f.app
        .world()
        .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
        .unwrap()
        .spawn(async move {
            let cancel = worker.cancel.clone();
            let join = tokio::spawn(async move {
                cancel.cancelled().await;
                while !flag.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
                ended.store(true, Ordering::Release);
            });
            worker.attach_task(join).unwrap();
            let _ = tx.send(());
        });
    f.until(&mut installed);
    drop(handle.close());
    for _ in 0..100 {
        f.tick();
        std::thread::yield_now();
    }
    assert!(inference.cancel.is_cancelled());
    assert!(f.storage.is_open());
    assert!(f.app.world().is_alive(entity));
    drain.completed(None);
    for _ in 0..50 {
        f.tick();
        std::thread::yield_now();
    }
    assert!(f.storage.is_open(), "inference worker is still cleaning up");
    released.store(true, Ordering::Release);
    f.close(handle).unwrap();
    assert!(stopped.load(Ordering::Acquire));
    assert!(!f.storage.is_open());
}

#[test]
fn start_rejects_foreign_workspace_parent() {
    let mut f = Fixture::new(quiet());
    let foreign = f.app.world_mut().spawn();
    f.app.world_mut().insert_component(
        foreign,
        Agent::new(foreign, foreign, "other".into(), f.root.path().into()),
    );
    f.app
        .world_mut()
        .insert_component(foreign, Address::new("/foreign/parent").unwrap());
    let (req, mut rx) = f.request();
    let mut req = req;
    req.parent = foreign;
    req.address = "/foreign/parent/coder".into();
    f.app.world().send_event(req);
    let failure = f.until(&mut rx).unwrap_err();
    assert_eq!(failure.kind, AgentErrorKind::InvalidRequest);
    assert!(
        f.storage.is_open(),
        "rejected request storage remains owned by creation supervisor"
    );
    f.app.world_mut().despawn(foreign);
}

#[test]
fn creation_failure_reply_follows_actual_rollback() {
    let release = Arc::new(AtomicBool::new(false));
    let flag = release.clone();
    let sync = AgentUpdateContext::new(
        move |_, _, kind| {
            if matches!(kind, AgentUpdateKind::SourcesInstalled { .. }) {
                let flag = flag.clone();
                vec![Box::pin(async move {
                    while !flag.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                    Ok(())
                })]
            } else {
                vec![]
            }
        },
        |_, _| {},
    );
    let mut f = Fixture::new(sync);
    let (req, mut rx) = f.request();
    f.app.world().send_event(req);
    for _ in 0..100 {
        f.tick();
        std::thread::yield_now();
    }
    assert!(
        matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
        "reply must await real rollback"
    );
    assert!(f.storage.is_open());
    assert_eq!(f.app.world().query_with::<Agent>().result().len(), 1);
    release.store(true, Ordering::Release);
    assert!(f.until(&mut rx).is_err());
    assert!(!f.storage.is_open());
    assert!(f.app.world().query_with::<Agent>().result().is_empty());
}
