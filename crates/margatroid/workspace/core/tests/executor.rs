use std::{
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
};
use tokio::sync::oneshot;
use workspace_core::*;

#[tokio::test]
async fn factories_and_non_send_futures_stay_on_one_dedicated_thread() {
    let executor = spawn_workspace_executor().unwrap();
    let caller = thread::current().id();
    let mut results = Vec::new();
    for n in 0..12 {
        let (send, receive) = oneshot::channel();
        executor
            .entry
            .submit(move || {
                let factory_thread = thread::current().id();
                let local = Rc::new(n);
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    assert_eq!(*local, n);
                    assert_eq!(factory_thread, thread::current().id());
                    send.send(factory_thread).unwrap();
                })
            })
            .unwrap();
        results.push(receive);
    }
    executor.close.close_and_join().await.unwrap();
    assert!(executor.close.is_joined());
    let owner = results.remove(0).await.unwrap();
    assert_ne!(owner, caller);
    for result in results {
        assert_eq!(result.await.unwrap(), owner);
    }
}
#[tokio::test]
async fn close_admission_does_not_stop_accepted_tasks_or_fake_join() {
    let executor = spawn_workspace_executor().unwrap();
    let old_entry = executor.entry.clone();
    let (release, wait) = oneshot::channel();
    let (done, finished) = oneshot::channel();
    executor
        .entry
        .submit(move || {
            Box::pin(async move {
                wait.await.unwrap();
                done.send(()).unwrap();
            })
        })
        .unwrap();
    executor.entry.close_admission();
    assert!(old_entry.is_closed());
    assert!(!executor.close.is_joined());
    assert_eq!(
        old_entry.submit(|| Box::pin(async {})).unwrap_err().kind,
        WorkspaceErrorKind::StateRejected
    );
    release.send(()).unwrap();
    finished.await.unwrap();
    assert!(!executor.close.is_joined());
    executor.close.close_and_join().await.unwrap();
}
#[tokio::test]
async fn concurrent_close_drains_every_accepted_task_and_returns_same_result() {
    let executor = spawn_workspace_executor().unwrap();
    let completed = Arc::new(AtomicUsize::new(0));
    let mut releases = Vec::new();
    for _ in 0..16 {
        let (release, wait) = oneshot::channel();
        releases.push(release);
        let completed = completed.clone();
        executor
            .entry
            .submit(move || {
                Box::pin(async move {
                    wait.await.unwrap();
                    completed.fetch_add(1, Ordering::SeqCst);
                })
            })
            .unwrap();
    }
    let first = executor.close.clone();
    let first = tokio::spawn(async move { first.close_and_join().await });
    let second = executor.close.clone();
    let second = tokio::spawn(async move { second.close_and_join().await });
    while !executor.entry.is_closed() {
        tokio::task::yield_now().await;
    }
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    assert_eq!(completed.load(Ordering::SeqCst), 0);
    for release in releases {
        release.send(()).unwrap();
    }
    assert_eq!(first.await.unwrap(), Ok(()));
    assert_eq!(second.await.unwrap(), Ok(()));
    assert_eq!(completed.load(Ordering::SeqCst), 16);
    assert_eq!(executor.close.close_and_join().await, Ok(()));
}
#[tokio::test]
async fn cancelled_close_finisher_does_not_lose_join_or_cancel_tasks() {
    let executor = spawn_workspace_executor().unwrap();
    let (release, wait) = oneshot::channel();
    let completed = Arc::new(AtomicUsize::new(0));
    let flag = completed.clone();
    executor
        .entry
        .submit(move || {
            Box::pin(async move {
                wait.await.unwrap();
                flag.store(1, Ordering::SeqCst);
            })
        })
        .unwrap();
    let first = executor.close.clone();
    let first = tokio::spawn(async move { first.close_and_join().await });
    while !executor.entry.is_closed() {
        tokio::task::yield_now().await;
    }
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(!executor.close.is_joined());
    release.send(()).unwrap();
    executor.close.close_and_join().await.unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert!(executor.close.is_joined());
}
#[tokio::test]
async fn task_and_factory_panics_report_real_errors_after_other_tasks_drain() {
    for factory_panics in [false, true] {
        let executor = spawn_workspace_executor().unwrap();
        let completed = Arc::new(AtomicUsize::new(0));
        let flag = completed.clone();
        executor
            .entry
            .submit(move || {
                if factory_panics {
                    panic!("factory failed");
                }
                Box::pin(async {
                    panic!("future failed");
                })
            })
            .unwrap();
        executor
            .entry
            .submit(move || {
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    flag.store(1, Ordering::SeqCst);
                })
            })
            .unwrap();
        let result = executor.close.close_and_join().await;
        let error = result.clone().unwrap_err();
        assert_eq!(error.kind, WorkspaceErrorKind::ThreadCloseFailed);
        assert!(error.message.contains(if factory_panics {
            "factory failed"
        } else {
            "future failed"
        }));
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(executor.close.close_and_join().await, result);
    }
}
#[tokio::test]
async fn component_drop_closes_admission_without_claiming_cleanup_or_cancelling_tasks() {
    let executor = spawn_workspace_executor().unwrap();
    let entry = executor.entry.clone();
    let close = executor.close.clone();
    let (release, wait) = oneshot::channel();
    let (done, finished) = oneshot::channel();
    executor
        .entry
        .submit(move || {
            Box::pin(async move {
                wait.await.unwrap();
                done.send(()).unwrap();
            })
        })
        .unwrap();
    drop(executor);
    assert!(entry.is_closed());
    assert!(!close.is_joined());
    release.send(()).unwrap();
    finished.await.unwrap();
    close.close_and_join().await.unwrap();
}
#[tokio::test]
async fn joining_from_workspace_thread_is_rejected_without_deadlock() {
    let executor = spawn_workspace_executor().unwrap();
    let close = executor.close.clone();
    let (send, receive) = oneshot::channel();
    executor
        .entry
        .submit(move || {
            Box::pin(async move {
                send.send(close.close_and_join().await).unwrap();
            })
        })
        .unwrap();
    assert_eq!(
        receive.await.unwrap().unwrap_err().kind,
        WorkspaceErrorKind::ThreadCloseFailed
    );
    executor.close.close_and_join().await.unwrap();
}
