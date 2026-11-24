use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tool_core::{
    TaskChildClose, ToolError, ToolErrorKind, ToolExecutionLimits, ToolTaskHandle, ToolTasks,
};
fn task(tasks: &ToolTasks) -> ToolTaskHandle {
    ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        tokio::sync::oneshot::channel().0,
        tasks.update_context(),
        None,
        Arc::new(|h| {
            Box::pin(async move {
                h.finish_children().await?;
                h.retry_cleanups().await
            })
        }),
    )
    .unwrap()
    .0
}
#[test]
fn independent_snapshot_observer_and_admission() {
    let tasks = ToolTasks::new();
    let source = tasks.snapshot_source();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let read = source.clone();
    let retired = Arc::new(AtomicUsize::new(0));
    let retirement = retired.clone();
    tasks.observe(
        Arc::new(move |version| {
            assert_eq!(read().unwrap().0, version);
            seen.lock().unwrap().push(version);
        }),
        Box::new(move || {
            retirement.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let handle = task(&tasks);
    tasks.register("one".into(), handle.clone()).unwrap();
    handle.write(serde_json::json!("text")).unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2);
    tasks.close_admission();
    assert!(!tasks.is_accepting());
    assert_eq!(
        tasks.register("two".into(), task(&tasks)).unwrap_err().kind,
        ToolErrorKind::Closed
    );
    assert_eq!(
        source().unwrap().1[0].output,
        vec![serde_json::json!("text")]
    );
    drop(tasks);
    assert!(source().is_none());
    assert_eq!(retired.load(Ordering::SeqCst), 1);
    drop(handle);
}
#[tokio::test]
async fn child_and_compensation_failures_remain_retryable_and_block_completion() {
    let tasks = ToolTasks::new();
    let handle = task(&tasks);
    tasks.register("one".into(), handle.clone()).unwrap();
    let mut world = core_plugin::World::new();
    let entity = world.spawn();
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let close: TaskChildClose = Arc::new(move || {
        let n = count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if n == 0 {
                Err(ToolError::new(ToolErrorKind::CleanupFailed, "retry"))
            } else {
                Ok(())
            }
        })
    });
    handle.bind_child(entity, close).unwrap();
    assert!(!tasks.complete_cleanup("one", &handle, None));
    assert!(handle.finish_children().await.is_err());
    assert!(
        handle
            .bind_child(world.spawn(), Arc::new(|| Box::pin(async { Ok(()) })))
            .is_err()
    );
    let retry = Arc::new(AtomicUsize::new(0));
    let count = retry.clone();
    handle
        .retain_cleanup(Arc::new(move || {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Err(ToolError::new(ToolErrorKind::CleanupFailed, "storage"))
                } else {
                    Ok(())
                }
            })
        }))
        .unwrap();
    assert!(handle.retry_cleanups().await.is_err());
    assert!(!tasks.complete_cleanup("one", &handle, None));
    handle.finish_children().await.unwrap();
    handle.retry_cleanups().await.unwrap();
    assert!(tasks.complete_cleanup("one", &handle, None));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(retry.load(Ordering::SeqCst), 2);
    assert!(
        handle
            .retain_cleanup(Arc::new(|| Box::pin(async { Ok(()) })))
            .is_err()
    );
}
#[tokio::test]
async fn finish_callback_is_requested_before_its_waiter_is_polled_or_dropped() {
    let tasks = ToolTasks::new();
    let requested = Arc::new(AtomicUsize::new(0));
    let count = requested.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(done_tx)));
    let (handle, _) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        tokio::sync::oneshot::channel().0,
        tasks.update_context(),
        None,
        Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            let sender = tx.lock().unwrap().take();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                if let Some(sender) = sender {
                    let _ = sender.send(());
                }
            });
            Box::pin(async { Ok(()) })
        }),
    )
    .unwrap();
    let waiter = handle.finish();
    assert!(handle.cancel_token().is_cancelled());
    assert_eq!(requested.load(Ordering::SeqCst), 1);
    drop(waiter);
    done_rx.await.unwrap();
}
#[test]
fn path_request_clones_share_the_unique_original_reply() {
    use tool_core::ToolExecuteRequest;
    let mut world = core_plugin::World::new();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let event = ToolExecuteRequest::Path {
        initiator: world.spawn(),
        path: "/tool.toml".into(),
        arguments: serde_json::json!({}),
        reply: Arc::new(Mutex::new(Some(tx))),
    };
    let clone = event.clone();
    let ToolExecuteRequest::Path { reply, .. } = clone else {
        unreachable!()
    };
    reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Err(ToolError::new(ToolErrorKind::Closed, "closed")))
        .unwrap();
    let ToolExecuteRequest::Path { reply, .. } = event else {
        unreachable!()
    };
    assert!(reply.lock().unwrap().is_none());
    assert_eq!(
        rx.try_recv().unwrap().unwrap_err().kind,
        ToolErrorKind::Closed
    );
}
