use serde_json::json;
use std::sync::{Arc, Barrier};
use tokio::sync::oneshot;
use tool_core::{ToolErrorKind, ToolExecutionLimits, ToolTaskHandle, ToolTasks};

fn owned(
    tasks: &ToolTasks,
    id: &str,
    owner: Option<&str>,
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
    )
    .unwrap();
    handle.set_owner_task_id(owner.map(str::to_owned));
    tasks.register(id.into(), handle.clone()).unwrap();
    (handle, receiver)
}

#[tokio::test]
async fn owner_cleanup_only_fails_pending_dependents_and_retains_live_handles() {
    let tasks = ToolTasks::new();
    let (pending, result) = owned(&tasks, "pending", Some("owner"));
    let (returned, completed) = owned(&tasks, "returned", Some("owner"));
    returned
        .return_result(Ok(tool_core::parse_tool_result(
            &json!("done"),
            &ToolExecutionLimits::default(),
        )
        .unwrap()))
        .unwrap();
    let (unrelated, _) = owned(&tasks, "unrelated", Some("other"));
    let (ordinary, _) = owned(&tasks, "ordinary", None);
    assert_eq!(tasks.abort_owner_unreturned("owner"), 1);
    assert_eq!(
        result.await.unwrap().unwrap_err().kind,
        ToolErrorKind::OwnerGone
    );
    assert_eq!(completed.await.unwrap().unwrap().call_id, "call-returned");
    assert!(pending.cancel_token().is_cancelled());
    assert!(!returned.cancel_token().is_cancelled());
    assert!(!unrelated.cancel_token().is_cancelled());
    assert!(!ordinary.cancel_token().is_cancelled());
    assert_eq!(tasks.list().len(), 4);
    assert_eq!(
        tasks.read("pending").unwrap().error.unwrap().kind,
        ToolErrorKind::OwnerGone
    );
    assert_eq!(tasks.abort_owner_unreturned("owner"), 0);
}

#[test]
fn owner_cleanup_and_return_share_one_atomic_reply() {
    for _ in 0..64 {
        let tasks = ToolTasks::new();
        let (handle, mut receiver) = owned(&tasks, "dependent", Some("owner"));
        let barrier = Arc::new(Barrier::new(2));
        let race = barrier.clone();
        let returning = handle.clone();
        let worker = std::thread::spawn(move || {
            race.wait();
            returning.return_result(Ok(tool_core::parse_tool_result(
                &json!("result"),
                &ToolExecutionLimits::default(),
            )
            .unwrap()))
        });
        barrier.wait();
        let cancelled = tasks.abort_owner_unreturned("owner");
        let returned = worker.join().unwrap();
        let reply = receiver.try_recv().unwrap();
        match reply {
            Ok(response) => {
                assert_eq!(response.call_id, "call-dependent");
                assert!(returned.is_ok());
                assert_eq!(cancelled, 0);
                assert!(!handle.cancel_token().is_cancelled());
            }
            Err(error) => {
                assert_eq!(error.kind, ToolErrorKind::OwnerGone);
                assert_eq!(returned.unwrap_err().kind, ToolErrorKind::AlreadyReturned);
                assert_eq!(cancelled, 1);
                assert!(handle.cancel_token().is_cancelled());
            }
        }
        assert!(handle.has_returned());
    }
}
