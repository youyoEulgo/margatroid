use super::*;
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;

fn fixture() -> (App, Arc<CompletionGuard>) {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default());
    let target = app.world_mut().spawn();
    let request = LoadAgentDriver {
        id: "guard".into(),
        target,
        resource_id: ResourceID::parse("driver:local/test").unwrap(),
    };
    let guard = Arc::new(CompletionGuard {
        request,
        events: app.world().event_sender(),
        completed: Arc::new(AtomicBool::new(false)),
    });
    (app, guard)
}
fn replies(app: &mut App) -> usize {
    app.tick();
    app.world()
        .event_reader::<LoadAgentDriverResult>()
        .into_iter()
        .filter(|r| r.id == "guard" && r.result.is_err())
        .count()
}
#[test]
fn cancelled_future_reports_once_when_last_capture_drops() {
    let (mut app, guard) = fixture();
    let duplicate = guard.clone();
    let future = async move {
        std::future::pending::<()>().await;
        drop(guard);
    };
    drop(future);
    assert_eq!(replies(&mut app), 0);
    drop(duplicate);
    assert_eq!(replies(&mut app), 1);
    assert_eq!(replies(&mut app), 0);
}
#[test]
fn panic_unwind_reports_once() {
    let (mut app, guard) = fixture();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = guard;
        panic!("worker panic");
    }));
    assert!(caught.is_err());
    assert_eq!(replies(&mut app), 1);
    assert_eq!(replies(&mut app), 0);
}
#[test]
fn dropped_output_retains_guard_until_last_clone() {
    let (mut app, guard) = fixture();
    let output = AgentDriverReadOutput {
        request: guard.request.clone(),
        result: Err(error("read failed")),
        completed: guard.completed.clone(),
        supervision: guard,
    };
    let duplicate = output.clone();
    drop(output);
    assert_eq!(replies(&mut app), 0);
    drop(duplicate);
    assert_eq!(replies(&mut app), 1);
}
#[test]
fn normal_failure_and_duplicate_output_reply_once() {
    let (mut app, guard) = fixture();
    let output = AgentDriverReadOutput {
        request: guard.request.clone(),
        result: Err(error("read failed")),
        completed: guard.completed.clone(),
        supervision: guard,
    };
    handle_mount_agent_driver(app.world_mut(), output.clone());
    handle_mount_agent_driver(app.world_mut(), output);
    assert_eq!(replies(&mut app), 1);
    assert_eq!(replies(&mut app), 0);
}
