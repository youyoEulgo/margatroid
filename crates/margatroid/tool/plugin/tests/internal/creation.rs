use super::*;

#[tokio::test]
async fn failed_agent_close_keeps_storage_and_cleanup_owned_until_retry() {
    let directory = tempfile::tempdir().unwrap();
    let storage = storage_core::open_storage(
        &directory.path().join("state.sqlite"),
        &directory.path().join("setting.toml"),
        "[custom]\nvalue=1\n",
    )
    .unwrap();
    let mut world = core_plugin::World::new();
    let entity = world.spawn();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts = calls.clone();
    let (_, ready) = tokio::sync::watch::channel(agent_core::AgentReadyState::Pending);
    let handle = agent_core::AgentHandle::new(entity, ready, move || {
        let first = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                Err(agent_core::AgentError::new(
                    agent_core::AgentErrorKind::CleanupFailed,
                    "child remains alive",
                ))
            } else {
                Ok(())
            }
        })
    });
    let state = Arc::new(Mutex::new(Compensation {
        pending_handoff: None,
        agent: Some(handle),
        storage: Some(storage.clone()),
        directory: directory.path().to_path_buf(),
        remove_directory: false,
        operation: None,
    }));
    assert!(compensate(state.clone()).await.is_err());
    assert!(
        storage.is_open(),
        "failed Agent close cannot close its live storage"
    );
    assert!(state.lock().unwrap().agent.is_some());
    compensate(state.clone()).await.unwrap();
    assert!(!storage.is_open());
    assert!(state.lock().unwrap().agent.is_none());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn execution_join_is_independent_of_cached_cleanup_failure() {
    let (reply, _receiver) = oneshot::channel();
    let tasks = tool_core::ToolTasks::new();
    let (handle, _input) = tool_core::ToolTaskHandle::new(
        &tool_core::ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        reply,
        tasks.update_context(),
        None,
        Arc::new(|_| Box::pin(async { Ok(()) })),
    )
    .unwrap();
    handle.attach_task(tokio::spawn(async {})).unwrap();
    handle.set_error(tool_failure("previous child cleanup failed"));
    handle.wait_execution_ended().await.unwrap();
    assert!(
        !handle.is_cleanup_complete(),
        "join cannot claim child cleanup finished"
    );
}

#[tokio::test]
async fn execution_join_waiter_wakes_when_supervisor_is_attached_later() {
    let (reply, _receiver) = oneshot::channel();
    let tasks = tool_core::ToolTasks::new();
    let (handle, _) = tool_core::ToolTaskHandle::new(
        &tool_core::ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        reply,
        tasks.update_context(),
        None,
        Arc::new(|_| Box::pin(async { Ok(()) })),
    )
    .unwrap();
    let waiting = handle.clone();
    let waiter = tokio::spawn(async move { waiting.wait_execution_ended().await });
    tokio::task::yield_now().await;
    handle.attach_task(tokio::spawn(async {})).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn failed_start_reply_uses_retained_child_close_before_storage_compensation() {
    use closure_plugin::{AppClosureExt, WorldClosureExt};
    let directory = tempfile::tempdir().unwrap();
    let storage = storage_core::open_storage(
        &directory.path().join("state.sqlite"),
        &directory.path().join("setting.toml"),
        "[custom]\nvalue=1\n",
    )
    .unwrap();
    let mut app = core_plugin::App::new();
    app.add_plugin(app_runtime_plugin::RuntimePlugin::default())
        .add_plugin(closure_plugin::ClosurePlugin)
        .add_closure_system(app_runtime_plugin::RuntimePlugin::UPDATE);
    let parent = app.world_mut().spawn();
    let child = app.world_mut().spawn();
    let tasks = tool_core::ToolTasks::new();
    let (tx, _) = oneshot::channel();
    let (task, _) = tool_core::ToolTaskHandle::new(
        &tool_core::ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        tx,
        tasks.update_context(),
        None,
        Arc::new(|_| Box::pin(async { Ok(()) })),
    )
    .unwrap();
    tasks.register("creator".into(), task.clone()).unwrap();
    app.world_mut().insert_component(parent, tasks);
    app.world_mut().insert_component(
        child,
        agent_core::Agent::new(parent, parent, "creator".into(), directory.path().into()),
    );
    app.world_mut()
        .insert_component(child, address::Address::new("/demo/coder").unwrap());
    app.world_mut().insert_component(child, storage.clone());
    task.retain_child_for_cleanup(
        child,
        Arc::new(|| Box::pin(async { Err(tool_failure("rollback has not cleaned live Agent")) })),
    )
    .unwrap();
    let sender = Arc::new(
        app.world()
            .closure_sender(app_runtime_plugin::RuntimePlugin::UPDATE),
    );
    let cleanup = Arc::new(Mutex::new(Compensation {
        agent: None,
        pending_handoff: Some(PendingAgentHandoff {
            world: Arc::downgrade(&sender),
            address: "/demo/coder".into(),
            parent,
            task_id: "creator".into(),
            storage: storage.clone(),
        }),
        storage: Some(storage.clone()),
        directory: directory.path().into(),
        remove_directory: false,
        operation: None,
    }));
    let check = compensate(cleanup.clone());
    tokio::pin!(check);
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
    tokio::pin!(deadline);
    let result = loop {
        app.tick();
        tokio::select! {result=&mut check=>break result,()=&mut deadline=>panic!("handoff lookup did not finish"),()=tokio::time::sleep(std::time::Duration::from_millis(1))=>{}}
    };
    assert!(result.is_err());
    assert!(
        storage.is_open(),
        "failed StartAgent reply cannot close a still-owned storage"
    );
    assert!(cleanup.lock().unwrap().pending_handoff.is_some());
    assert!(app.world().is_alive(child));
}
