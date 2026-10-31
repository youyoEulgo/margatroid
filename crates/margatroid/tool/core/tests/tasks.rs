use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use tokio::sync::oneshot;
use tool_core::{ToolError, ToolErrorKind, ToolExecutionLimits, ToolTaskHandle, ToolTasks};

fn handle() -> (ToolTaskHandle, tokio::sync::mpsc::Receiver<Value>) {
    ToolTaskHandle::new(&ToolExecutionLimits::default()).unwrap()
}

#[test]
fn cloned_collections_and_handles_share_outputs_and_cursor() {
    let tasks = ToolTasks::new();
    let (task, _input) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let worker = tasks.clone();
    std::thread::spawn(move || {
        let task = worker.get("task").unwrap();
        task.write(json!({"step": 1})).unwrap();
        task.write(json!([2, true])).unwrap();
    })
    .join()
    .unwrap();
    let first = tasks.read("task").unwrap();
    assert_eq!(first.output, [json!({"step": 1}), json!([2, true])]);
    assert!(!first.replaced);
    assert!(!first.completed);
    assert!(first.error.is_none());
    assert!(task.read().output.is_empty());
    task.write(json!("later")).unwrap();
    assert_eq!(tasks.read("task").unwrap().output, [json!("later")]);
}

#[test]
fn duplicate_ids_do_not_replace_existing_tasks_and_agents_are_isolated() {
    let tasks = ToolTasks::new();
    let other_agent = ToolTasks::new();
    let (first, _) = handle();
    first.write(json!("original")).unwrap();
    tasks.register("same".into(), first.clone()).unwrap();
    let (second, _) = handle();
    assert_eq!(
        tasks
            .register("same".into(), second.clone())
            .unwrap_err()
            .kind,
        ToolErrorKind::DuplicateTask
    );
    other_agent.register("same".into(), second).unwrap();
    assert_eq!(tasks.read("same").unwrap().output, [json!("original")]);
    assert!(other_agent.read("same").unwrap().output.is_empty());
    assert_eq!(tasks.list().len(), 1);
    assert_eq!(
        tasks.read("missing").unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        tasks.cancel("missing").unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        tasks.send("missing", json!(null)).unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        tasks.register(" ".into(), first).unwrap_err().kind,
        ToolErrorKind::InvalidRequest
    );
}

#[test]
fn replacements_reset_cursor_and_replaced_flag_is_consumed_once() {
    let (task, _) = handle();
    task.write(json!(1)).unwrap();
    task.write(json!(2)).unwrap();
    assert_eq!(task.read().output, [json!(1), json!(2)]);
    task.replace(json!({"screen": "one"})).unwrap();
    task.replace(json!({"screen": "two"})).unwrap();
    task.write(json!("tail")).unwrap();
    let read = task.read();
    assert!(read.replaced);
    assert_eq!(read.output, [json!({"screen": "two"}), json!("tail")]);
    assert!(!task.read().replaced);
    task.replace(json!("after-read")).unwrap();
    assert_eq!(task.read().output, [json!("after-read")]);
}

#[test]
fn cache_budget_errors_preserve_unread_values_and_replace_recovers_space() {
    let limits = ToolExecutionLimits {
        cache_bytes: 8,
        ..ToolExecutionLimits::default()
    };
    let (task, _) = ToolTaskHandle::new(&limits).unwrap();
    task.write(json!("abc")).unwrap();
    assert_eq!(
        task.write(json!("de")).unwrap_err().kind,
        ToolErrorKind::OutputTooLarge
    );
    assert_eq!(
        task.replace(json!("too large")).unwrap_err().kind,
        ToolErrorKind::OutputTooLarge
    );
    let first = task.read();
    assert_eq!(first.output, [json!("abc")]);
    assert!(!first.replaced);
    assert_eq!(
        task.write(json!("de")).unwrap_err().kind,
        ToolErrorKind::OutputTooLarge
    );
    task.replace(json!(null)).unwrap();
    task.write(json!(true)).unwrap();
    assert_eq!(task.read().output, [json!(null), json!(true)]);
}

#[test]
fn concurrent_reads_deliver_every_value_once_across_shared_cursor() {
    let tasks = ToolTasks::new();
    let (task, _) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    for number in 0..100 {
        task.write(json!(number)).unwrap();
    }
    let barrier = Arc::new(Barrier::new(8));
    let readers = (0..8)
        .map(|_| {
            let tasks = tasks.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                tasks.read("task").unwrap().output
            })
        })
        .collect::<Vec<_>>();
    let mut numbers = readers
        .into_iter()
        .flat_map(|reader| reader.join().unwrap())
        .map(|value| value.as_u64().unwrap())
        .collect::<Vec<_>>();
    numbers.sort_unstable();
    assert_eq!(numbers, (0..100).collect::<Vec<_>>());
}

#[tokio::test]
async fn input_queue_reports_full_closed_and_oversized_without_dropping_values() {
    let limits = ToolExecutionLimits {
        input_capacity: 1,
        argument_bytes: 8,
        ..ToolExecutionLimits::default()
    };
    let (task, mut input) = ToolTaskHandle::new(&limits).unwrap();
    task.send(json!({"x": 1})).unwrap();
    assert_eq!(
        task.send(json!(2)).unwrap_err().kind,
        ToolErrorKind::InputFull
    );
    assert_eq!(input.recv().await, Some(json!({"x": 1})));
    assert_eq!(
        task.send(json!("oversized")).unwrap_err().kind,
        ToolErrorKind::OutputTooLarge
    );
    task.send(json!(2)).unwrap();
    assert_eq!(input.recv().await, Some(json!(2)));
    drop(input);
    assert_eq!(
        task.send(json!(3)).unwrap_err().kind,
        ToolErrorKind::InputClosed
    );
}

#[test]
fn returning_or_dropping_original_reply_does_not_cancel_or_complete_task() {
    let tasks = ToolTasks::new();
    let (task, mut input) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let (sender, receiver) = oneshot::channel::<()>();
    drop(receiver);
    task.claim_return().unwrap();
    assert!(sender.send(()).is_err());
    assert!(task.has_returned());
    assert_eq!(
        task.claim_return().unwrap_err().kind,
        ToolErrorKind::AlreadyReturned
    );
    assert!(!task.cancel_token().is_cancelled());
    assert!(!task.is_completed());
    task.write(json!("background output")).unwrap();
    tasks.send("task", json!("still accepting input")).unwrap();
    assert_eq!(input.try_recv().unwrap(), json!("still accepting input"));
    drop(tasks);
    assert!(!task.cancel_token().is_cancelled());
    assert_eq!(task.read().output, [json!("background output")]);
}

#[tokio::test]
async fn finish_waits_for_real_cleanup_with_concurrent_waiters_and_retains_entry() {
    let tasks = ToolTasks::new();
    let (task, _) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let (cancel_seen, cancelled) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    let supervisor = task.clone();
    task.attach_task(tokio::spawn(async move {
        supervisor.cancel_token().cancelled().await;
        cancel_seen.send(()).unwrap();
        cleanup.await.unwrap();
        supervisor.write(json!("final output")).unwrap();
        supervisor.complete(None);
    }))
    .unwrap();
    let first_tasks = tasks.clone();
    let first = tokio::spawn(async move { first_tasks.finish("task").await });
    let second_tasks = tasks.clone();
    let second = tokio::spawn(async move { second_tasks.finish("task").await });
    cancelled.await.unwrap();
    assert!(!task.is_completed());
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    })
    .await
    .unwrap();
    assert_eq!(tasks.list().len(), 1);
    let read = tasks.read("task").unwrap();
    assert_eq!(read.output, [json!("final output")]);
    assert!(read.completed);
    tasks.finish("task").await.unwrap();
    assert_eq!(
        task.write(json!(1)).unwrap_err().kind,
        ToolErrorKind::Closed
    );
    assert_eq!(
        task.replace(json!(1)).unwrap_err().kind,
        ToolErrorKind::Closed
    );
    assert_eq!(
        task.send(json!(1)).unwrap_err().kind,
        ToolErrorKind::InputClosed
    );
    assert_eq!(task.claim_return().unwrap_err().kind, ToolErrorKind::Closed);
}

#[tokio::test]
async fn aborting_finish_waiter_does_not_drop_supervisor_control() {
    let (task, _) = handle();
    let (cancel_seen, cancelled) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    let supervisor = task.clone();
    task.attach_task(tokio::spawn(async move {
        supervisor.cancel_token().cancelled().await;
        cancel_seen.send(()).unwrap();
        cleanup.await.unwrap();
        supervisor.complete(None);
    }))
    .unwrap();
    let waiter_task = task.clone();
    let waiter = tokio::spawn(async move { waiter_task.finish().await });
    cancelled.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!task.is_completed());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task.finish())
        .await
        .unwrap()
        .unwrap();
    assert!(task.is_completed());
}

#[tokio::test]
async fn finish_before_supervisor_attachment_does_not_lose_notifications() {
    let (task, _) = handle();
    let waiter_task = task.clone();
    let waiter = tokio::spawn(async move { waiter_task.finish().await });
    task.cancel_token().cancelled().await;
    let supervisor = task.clone();
    task.attach_task(tokio::spawn(async move {
        supervisor.complete(None);
    }))
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cleanup_failures_preserve_management_and_do_not_fake_completion() {
    let (task, _) = handle();
    task.write(json!("retained")).unwrap();
    let error = ToolError::new(ToolErrorKind::CleanupFailed, "child remains alive");
    task.set_error(error.clone());
    let result = tokio::time::timeout(Duration::from_secs(1), task.finish())
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), error);
    assert!(!task.is_completed());
    let read = task.read();
    assert_eq!(read.output, [json!("retained")]);
    assert_eq!(read.error, Some(error));
    task.complete(None);
    task.finish().await.unwrap();
}

#[tokio::test]
async fn panicked_or_unconfirmed_supervisors_report_errors_without_completed() {
    let (panicked, _) = handle();
    panicked
        .attach_task(tokio::spawn(async {
            panic!("test supervisor panic");
        }))
        .unwrap();
    assert_eq!(
        panicked.finish().await.unwrap_err().kind,
        ToolErrorKind::TaskPanicked
    );
    assert!(!panicked.is_completed());
    let (unconfirmed, _) = handle();
    unconfirmed.attach_task(tokio::spawn(async {})).unwrap();
    assert_eq!(
        unconfirmed.finish().await.unwrap_err().kind,
        ToolErrorKind::CleanupFailed
    );
    assert!(!unconfirmed.is_completed());
}

#[test]
fn completion_preserves_error_and_is_idempotent() {
    let (task, _) = handle();
    let error = ToolError::new(ToolErrorKind::ExecutionFailed, "background failure");
    task.set_error(error.clone());
    task.complete(None);
    task.complete(Some(ToolError::new(
        ToolErrorKind::Cancelled,
        "late completion",
    )));
    assert_eq!(task.read().error, Some(error));
    assert!(task.read().completed);
}
