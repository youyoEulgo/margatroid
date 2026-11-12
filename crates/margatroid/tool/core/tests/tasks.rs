use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use tokio::sync::oneshot;
use tool_core::{ToolError, ToolErrorKind, ToolExecutionLimits, ToolTaskHandle, ToolTasks};

fn handle() -> (ToolTaskHandle, tokio::sync::mpsc::UnboundedReceiver<Value>) {
    with_limits(&ToolExecutionLimits::default())
}

fn with_limits(
    limits: &ToolExecutionLimits,
) -> (ToolTaskHandle, tokio::sync::mpsc::UnboundedReceiver<Value>) {
    let (reply, _) = oneshot::channel();
    ToolTaskHandle::new(limits, "call".into(), "tool".into(), reply).unwrap()
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
    assert!(first.error.is_none());
    assert!(task.read().unwrap().output.is_empty());
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
    assert_eq!(task.read().unwrap().output, [json!(1), json!(2)]);
    task.replace(json!({"screen": "one"})).unwrap();
    task.replace(json!({"screen": "two"})).unwrap();
    task.write(json!("tail")).unwrap();
    let read = task.read().unwrap();
    assert!(read.replaced);
    assert_eq!(read.output, [json!({"screen": "two"}), json!("tail")]);
    assert!(!task.read().unwrap().replaced);
    task.replace(json!("after-read")).unwrap();
    assert_eq!(task.read().unwrap().output, [json!("after-read")]);
}

#[test]
fn rolling_cache_evicts_old_entries_and_preserves_the_shared_cursor() {
    let limits = ToolExecutionLimits {
        cache_bytes: 8,
        ..Default::default()
    };
    let (task, _) = with_limits(&limits);
    let (other, _) = with_limits(&limits);
    other.write(json!("safe")).unwrap();
    task.write(json!(11)).unwrap();
    task.write(json!(22)).unwrap();
    assert_eq!(task.read().unwrap().output, [json!(11), json!(22)]);
    task.write(json!(3333)).unwrap();
    task.write(json!(4444)).unwrap();
    let read = task.read().unwrap();
    assert_eq!(read.output, [json!(3333), json!(4444)]);
    assert!(read.error.is_none());
    assert!(!read.replaced);
    assert!(task.read().unwrap().output.is_empty());
    task.write(json!(5555)).unwrap();
    assert_eq!(task.read().unwrap().output, [json!(5555)]);
    assert_eq!(other.read().unwrap().output, [json!("safe")]);
}

#[test]
fn unread_cache_entries_roll_out_and_oversized_structured_values_do_not_break_the_task() {
    let limits = ToolExecutionLimits {
        cache_bytes: 8,
        ..Default::default()
    };
    let (task, _) = with_limits(&limits);
    task.write(json!(1111)).unwrap();
    task.write(json!(2222)).unwrap();
    task.write(json!(3333)).unwrap();
    assert_eq!(task.read().unwrap().output, [json!(2222), json!(3333)]);
    task.write(json!({"larger": [1, 2, 3]})).unwrap();
    let read = task.read().unwrap();
    assert!(read.output.is_empty());
    assert!(read.error.is_none());
    task.write(json!(42)).unwrap();
    assert_eq!(task.read().unwrap().output, [json!(42)]);
}

#[test]
fn escaped_cache_text_fits_the_serialized_window_and_keeps_its_tail() {
    let limits = ToolExecutionLimits {
        cache_bytes: 16,
        ..Default::default()
    };
    let (task, _) = with_limits(&limits);
    let source = "old\n\u{0}\"\\你好\n界";
    task.write(json!(source)).unwrap();
    let read = task.read().unwrap();
    let text = read.output[0].as_str().unwrap();
    assert!(source.ends_with(text));
    assert!(text.ends_with('界'));
    assert!(serde_json::to_vec(&read.output[0]).unwrap().len() <= limits.cache_bytes);
    assert!(read.error.is_none());
}

#[test]
fn overlong_cache_text_retains_a_valid_utf8_tail_and_can_be_replaced() {
    let limits = ToolExecutionLimits {
        cache_bytes: 8,
        ..Default::default()
    };
    let (task, _) = with_limits(&limits);
    task.write(json!("discard你好世界")).unwrap();
    let read = task.read().unwrap();
    let text = read.output[0].as_str().unwrap();
    assert!("discard你好世界".ends_with(text));
    assert!(text.ends_with('界'));
    assert!(text.len() <= 8);
    assert!(read.error.is_none());
    task.replace(json!(null)).unwrap();
    task.write(json!(true)).unwrap();
    let read = task.read().unwrap();
    assert_eq!(read.output, [json!(null), json!(true)]);
    assert!(read.replaced);
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
async fn input_queue_preserves_all_pending_values_and_reports_closed() {
    let (task, mut input) = handle();
    let values = (0..256)
        .map(|index| json!({"index": index, "text": "payload".repeat(32)}))
        .collect::<Vec<_>>();
    for value in &values {
        task.send(value.clone()).unwrap();
    }
    for value in values {
        assert_eq!(input.recv().await, Some(value));
    }
    drop(input);
    assert_eq!(
        task.send(json!(3)).unwrap_err().kind,
        ToolErrorKind::InputClosed
    );
}

fn response() -> tool_core::ToolExecuteResponse {
    tool_core::ToolExecuteResponse {
        call_id: "wrong-call".into(),
        alias: "wrong-alias".into(),
        content: "normal".into(),
        images: vec![],
        failed: false,
    }
}

#[tokio::test]
async fn return_delivers_original_association_and_keeps_background_task_alive() {
    let tasks = ToolTasks::new();
    let (reply, result) = oneshot::channel();
    let (task, mut input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "original-call".into(),
        "original-alias".into(),
        reply,
    )
    .unwrap();
    tasks.register("task".into(), task.clone()).unwrap();
    task.return_result(Ok(response())).unwrap();
    let returned = result.await.unwrap().unwrap();
    assert_eq!(returned.call_id, "original-call");
    assert_eq!(returned.alias, "original-alias");
    assert_eq!(
        task.return_result(Ok(response())).unwrap_err().kind,
        ToolErrorKind::AlreadyReturned
    );
    assert_eq!(tasks.abort_unreturned(), 0);
    assert!(!task.cancel_token().is_cancelled());
    task.write(json!("background")).unwrap();
    tasks.send("task", json!("input")).unwrap();
    assert_eq!(input.recv().await, Some(json!("input")));
    assert_eq!(tasks.read("task").unwrap().output, [json!("background")]);
    assert_eq!(tasks.list().len(), 1);
}

#[test]
fn abort_and_return_race_delivers_exactly_one_reply_and_cancels_only_abort_winner() {
    for _ in 0..128 {
        let tasks = ToolTasks::new();
        let (reply, mut result) = oneshot::channel();
        let (task, _input) = ToolTaskHandle::new(
            &ToolExecutionLimits::default(),
            "call".into(),
            "alias".into(),
            reply,
        )
        .unwrap();
        tasks.register("task".into(), task.clone()).unwrap();
        let gate = Arc::new(Barrier::new(2));
        let worker = task.clone();
        let worker_gate = gate.clone();
        let normal = std::thread::spawn(move || {
            worker_gate.wait();
            worker.return_result(Ok(response()))
        });
        gate.wait();
        let aborted = tasks.abort_unreturned();
        let normal = normal.join().unwrap();
        let delivered = result.try_recv().unwrap().unwrap();
        assert_eq!(delivered.call_id, "call");
        assert_eq!(delivered.alias, "alias");
        assert!(task.has_returned());
        assert_eq!(tasks.abort_unreturned(), 0);
        if aborted == 1 {
            assert_eq!(normal.unwrap_err().kind, ToolErrorKind::AlreadyReturned);
            assert_eq!(delivered.content, "Tool call aborted.");
            assert!(delivered.failed);
            assert!(task.cancel_token().is_cancelled());
        } else {
            normal.unwrap();
            assert_eq!(delivered.content, "normal");
            assert!(!delivered.failed);
            assert!(!task.cancel_token().is_cancelled());
        }
    }
}

#[tokio::test]
async fn abort_is_immediate_and_does_not_touch_returned_tasks() {
    let tasks = ToolTasks::new();
    let (pending_reply, pending_result) = oneshot::channel();
    let (pending, _) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "pending-call".into(),
        "pending-tool".into(),
        pending_reply,
    )
    .unwrap();
    tasks.register("pending".into(), pending.clone()).unwrap();
    let (returned, mut input) = handle();
    tasks.register("returned".into(), returned.clone()).unwrap();
    returned.return_result(Ok(response())).unwrap();
    returned.write(json!("retained")).unwrap();
    assert_eq!(tasks.abort_unreturned(), 1);
    let result = pending_result.await.unwrap().unwrap();
    assert_eq!(
        (result.call_id.as_str(), result.alias.as_str()),
        ("pending-call", "pending-tool")
    );
    assert!(result.failed);
    assert!(pending.cancel_token().is_cancelled());
    assert!(!pending.is_cleanup_complete());
    assert!(!returned.cancel_token().is_cancelled());
    assert_eq!(tasks.read("returned").unwrap().output, [json!("retained")]);
    tasks.send("returned", json!(5)).unwrap();
    assert_eq!(input.recv().await, Some(json!(5)));
    assert_eq!(tasks.list().len(), 2);
}

#[tokio::test]
async fn finish_waits_for_real_cleanup_then_removes_task_and_cache_for_all_waiters() {
    let tasks = ToolTasks::new();
    let (task, _input) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let (cancel_seen, cancelled) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    let supervisor = task.clone();
    let collection = tasks.clone();
    task.attach_task(tokio::spawn(async move {
        supervisor.cancel_token().cancelled().await;
        cancel_seen.send(()).unwrap();
        cleanup.await.unwrap();
        supervisor.write(json!("final output")).unwrap();
        collection.complete_cleanup("task", &supervisor, None);
    }))
    .unwrap();
    let first_task = task.clone();
    let first = tokio::spawn(async move { first_task.finish().await });
    let second_task = task.clone();
    let second = tokio::spawn(async move { second_task.finish().await });
    cancelled.await.unwrap();
    assert!(!task.is_cleanup_complete());
    assert_eq!(tasks.list().len(), 1);
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    })
    .await
    .unwrap();
    assert!(tasks.list().is_empty());
    assert_eq!(
        tasks.read("task").unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        tasks.send("task", json!(1)).unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        tasks.finish("task").await.unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(task.read().unwrap_err().kind, ToolErrorKind::TaskMissing);
    assert_eq!(
        task.write(json!(1)).unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        task.replace(json!(1)).unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    assert_eq!(
        task.send(json!(1)).unwrap_err().kind,
        ToolErrorKind::TaskMissing
    );
    task.finish().await.unwrap();
}

#[tokio::test]
async fn aborting_finish_waiter_retains_supervision_until_cleanup() {
    let tasks = ToolTasks::new();
    let (task, _) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let (cancel_seen, cancelled) = oneshot::channel();
    let (release, cleanup) = oneshot::channel();
    let supervisor = task.clone();
    let collection = tasks.clone();
    task.attach_task(tokio::spawn(async move {
        supervisor.cancel_token().cancelled().await;
        cancel_seen.send(()).unwrap();
        cleanup.await.unwrap();
        collection.complete_cleanup("task", &supervisor, None);
    }))
    .unwrap();
    let waiter_task = task.clone();
    let waiter = tokio::spawn(async move { waiter_task.finish().await });
    cancelled.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!task.is_cleanup_complete());
    assert_eq!(tasks.list().len(), 1);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task.finish())
        .await
        .unwrap()
        .unwrap();
    assert!(tasks.list().is_empty());
}

#[tokio::test]
async fn finish_before_attachment_does_not_lose_notifications() {
    let tasks = ToolTasks::new();
    let (task, _) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    let waiter_task = task.clone();
    let waiter = tokio::spawn(async move { waiter_task.finish().await });
    task.cancel_token().cancelled().await;
    let supervisor = task.clone();
    task.attach_task(tokio::spawn(async move {
        tasks.complete_cleanup("task", &supervisor, None);
    }))
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cleanup_failure_preserves_entry_cache_and_management_until_recovery() {
    let tasks = ToolTasks::new();
    let (task, _) = handle();
    tasks.register("task".into(), task.clone()).unwrap();
    task.write(json!("retained")).unwrap();
    let error = ToolError::new(ToolErrorKind::CleanupFailed, "child remains alive");
    task.set_error(error.clone());
    assert_eq!(tasks.finish("task").await.unwrap_err(), error);
    assert!(!task.is_cleanup_complete());
    assert_eq!(tasks.list().len(), 1);
    let read = tasks.read("task").unwrap();
    assert_eq!(read.output, [json!("retained")]);
    assert_eq!(read.error, Some(error));
    tasks.complete_cleanup("task", &task, None);
    task.finish().await.unwrap();
    assert!(tasks.list().is_empty());
}

#[tokio::test]
async fn panicked_or_unconfirmed_supervisors_report_error_without_cleanup_completion() {
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
    assert!(!panicked.is_cleanup_complete());
    let (unconfirmed, _) = handle();
    unconfirmed.attach_task(tokio::spawn(async {})).unwrap();
    assert_eq!(
        unconfirmed.finish().await.unwrap_err().kind,
        ToolErrorKind::CleanupFailed
    );
    assert!(!unconfirmed.is_cleanup_complete());
}

#[tokio::test]
async fn supervisor_cannot_join_itself() {
    let (task, _) = handle();
    let supervisor = task.clone();
    let (start, started) = oneshot::channel();
    let (reply, result) = oneshot::channel();
    task.attach_task(tokio::spawn(async move {
        started.await.unwrap();
        reply.send(supervisor.finish().await).unwrap();
    }))
    .unwrap();
    start.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), result)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        ToolErrorKind::InvalidRequest
    );
}

#[test]
fn late_supervisor_cleanup_cannot_remove_replacement_with_same_id() {
    let tasks = ToolTasks::new();
    let (old, _) = handle();
    tasks.register("task".into(), old.clone()).unwrap();
    assert!(tasks.complete_cleanup("task", &old, None));
    let (replacement, _) = handle();
    replacement.write(json!("replacement")).unwrap();
    tasks.register("task".into(), replacement.clone()).unwrap();
    assert!(!tasks.complete_cleanup("task", &old, None));
    assert_eq!(tasks.list().len(), 1);
    assert!(!replacement.is_cleanup_complete());
    assert_eq!(tasks.read("task").unwrap().output, [json!("replacement")]);
}

#[test]
fn reads_join_unread_text_without_changing_structured_output_or_cursor() {
    let (task, _) = handle();
    for value in [
        json!("你好"),
        json!("\n"),
        json!("world"),
        json!({"exit_code":0}),
        json!("after"),
        json!(" status"),
        json!([1, 2]),
    ] {
        task.write(value).unwrap();
    }
    assert_eq!(
        task.read().unwrap().output,
        [
            json!("你好\nworld"),
            json!({"exit_code":0}),
            json!("after status"),
            json!([1, 2])
        ]
    );
    assert!(task.read().unwrap().output.is_empty());
    task.write(json!("new")).unwrap();
    task.write(json!(" output")).unwrap();
    assert_eq!(task.read().unwrap().output, [json!("new output")]);
}

#[test]
fn snapshots_preserve_script_cursor_replaced_and_skip_cleanup() {
    let tasks = ToolTasks::new();
    let (task, _input) = handle();
    tasks.register("z".into(), task.clone()).unwrap();
    task.write(json!("first")).unwrap();
    task.read().unwrap();
    task.write(json!({"later":true})).unwrap();
    let snap = tasks.snapshot();
    assert_eq!(snap[0].task_id, "z");
    assert_eq!(snap[0].call_id, "call");
    assert_eq!(snap[0].alias, "tool");
    assert!(!snap[0].returned);
    assert_eq!(snap[0].output, vec![json!("first"), json!({"later":true})]);
    assert_eq!(task.read().unwrap().output, vec![json!({"later":true})]);
    task.replace(json!("replacement")).unwrap();
    assert_eq!(
        task.snapshot("z").unwrap().output,
        vec![json!("replacement")]
    );
    assert!(task.read().unwrap().replaced);
    task.set_error(ToolError::new(ToolErrorKind::ExecutionFailed, "failure"));
    assert!(tasks.snapshot()[0].error.is_some());
    tasks.complete_cleanup("z", &task, None);
    assert!(tasks.snapshot().is_empty());
    assert!(task.snapshot("z").is_none());
}
