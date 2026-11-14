use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};
use tool_core::TaskScope;
#[test]
fn scope_closes_once_and_refuses_late_bindings() {
    let scope = TaskScope::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let _binding = scope
        .bind(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let available = scope.availability();
    assert!(available());
    scope.close();
    scope.close();
    assert!(!available());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(scope.bind(|| {}).is_err());
}
#[test]
fn dropping_registration_unlinks_it_without_retaining_the_task() {
    let scope = TaskScope::default();
    let available = scope.availability();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let binding = scope
        .bind(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    drop(binding);
    drop(scope);
    assert!(!available());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[test]
fn dropping_last_task_scope_revokes_live_registrations() {
    let scope = TaskScope::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let _binding = scope
        .bind(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    drop(scope);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn binding_racing_with_close_is_either_rejected_or_revoked() {
    for _ in 0..100 {
        let scope = TaskScope::default();
        let other = scope.clone();
        let barrier = Arc::new(Barrier::new(2));
        let entering = barrier.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let thread = std::thread::spawn(move || {
            entering.wait();
            other.bind(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        });
        barrier.wait();
        scope.close();
        let binding = thread.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(binding.is_ok()));
    }
}
