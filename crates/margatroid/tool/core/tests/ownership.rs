use serde_json::json;
use std::sync::{Arc, Barrier};
use tokio::sync::oneshot;
use tool_core::{ToolErrorKind, ToolExecutionLimits, ToolTaskHandle, ToolTasks};

fn task(
    tasks: &ToolTasks,
    id: &str,
) -> (
    ToolTaskHandle,
    oneshot::Receiver<Result<tool_core::ToolExecuteResponse, tool_core::ToolError>>,
) {
    let (reply, receiver) = oneshot::channel();
    let (handle, _) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        format!("call-{id}"),
        format!("alias-{id}"),
        reply,
        tasks.update_context(),
    )
    .unwrap();
    tasks.register(id.into(), handle.clone()).unwrap();
    (handle, receiver)
}

#[tokio::test]
async fn cleaning_up_one_task_leaves_other_pending_and_returned_tasks_independent() {
    let tasks = test_tasks();
    let (owner, _) = task(&tasks, "owner");
    let (pending, mut result) = task(&tasks, "pending");
    let (returned, completed) = task(&tasks, "returned");
    returned
        .return_result(Ok(tool_core::parse_tool_result(&json!("done")).unwrap()))
        .unwrap();
    owner.cancel();
    assert!(tasks.complete_cleanup("owner", &owner, None));
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(completed.await.unwrap().unwrap().call_id, "call-returned");
    assert!(!pending.cancel_token().is_cancelled());
    assert!(!returned.cancel_token().is_cancelled());
    assert_eq!(tasks.list().len(), 2);
    assert!(tasks.read("pending").unwrap().error.is_none());
}

#[test]
fn abort_and_return_share_one_atomic_reply() {
    for _ in 0..64 {
        let tasks = test_tasks();
        let (handle, mut receiver) = task(&tasks, "pending");
        let barrier = Arc::new(Barrier::new(2));
        let race = barrier.clone();
        let returning = handle.clone();
        let worker = std::thread::spawn(move || {
            race.wait();
            returning.return_result(Ok(tool_core::parse_tool_result(&json!("result")).unwrap()))
        });
        barrier.wait();
        let cancelled = tasks.abort_unreturned();
        let returned = worker.join().unwrap();
        let response = receiver.try_recv().unwrap().unwrap();
        assert_eq!(response.call_id, "call-pending");
        if response.failed {
            assert_eq!(response.content, "Tool call aborted.");
            assert_eq!(returned.unwrap_err().kind, ToolErrorKind::AlreadyReturned);
            assert_eq!(cancelled, 1);
            assert!(handle.cancel_token().is_cancelled());
        } else {
            assert!(returned.is_ok());
            assert_eq!(cancelled, 0);
            assert!(!handle.cancel_token().is_cancelled());
        }
        assert!(handle.has_returned());
        assert_eq!(tasks.abort_unreturned(), 0);
    }
}

fn test_binding() -> tool_core::TaskSyncBinding {
    tool_core::TaskSyncBinding::new(
        client_core::Clients::default(),
        std::sync::Arc::from([]),
        |snapshot| {
            serde_json::to_string(&snapshot)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::EncodeFailed)
        },
    )
}
fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let owner = app.world_mut().spawn();
    tool_core::ToolTasks::new(owner, "agent:test/unit:latest".into(), test_binding())
}
