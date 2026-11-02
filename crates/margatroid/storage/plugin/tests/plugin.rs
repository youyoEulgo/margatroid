use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_core::*;
use storage_plugin::StoragePlugin;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(StoragePlugin::new());
    app
}
fn request(
    app: &App,
    owner: Entity,
    operation: StorageOperation,
    cancel: CancellationToken,
) -> oneshot::Receiver<Result<StorageValue, StorageError>> {
    let (tx, rx) = oneshot::channel();
    app.world().emit_event(StorageRequest {
        owner,
        operation,
        cancel,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    rx
}
fn wait(
    app: &mut App,
    rx: &mut oneshot::Receiver<Result<StorageValue, StorageError>>,
) -> Result<StorageValue, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.tick();
        match rx.try_recv() {
            Ok(result) => return result,
            Err(oneshot::error::TryRecvError::Closed) => panic!("reply lost"),
            Err(_) => {}
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn oneshot_worker_persists_and_cancel_after_commit_keeps_success() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    let cfg = dir.path().join("cfg");
    let storage = open_storage(&db, &cfg, "").unwrap();
    let mut app = app();
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, storage);
    let cancel = CancellationToken::new();
    let mut rx = request(
        &app,
        owner,
        StorageOperation::CreateBlock {
            name: "b".into(),
            fields: vec!["m".into()],
            instance: None,
        },
        cancel.clone(),
    );
    assert_eq!(wait(&mut app, &mut rx).unwrap(), StorageValue::Unit);
    cancel.cancel();
    drop(app);
    let reopened = open_storage(&db, &cfg, "").unwrap();
    assert_eq!(
        execute_storage(
            &reopened,
            StorageOperation::ReadMessages {
                selectors: vec![MessageSelector {
                    block: "b".into(),
                    field: "m".into(),
                    instance: None,
                    selection: MessageSelection::All
                }]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![])
    );
}
#[test]
fn generation_mismatch_and_cancelled_request_never_execute() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let mut app = app();
    let old = app.world_mut().spawn();
    app.world_mut().despawn(old);
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, storage.clone());
    let operation = StorageOperation::CreateBlock {
        name: "b".into(),
        fields: vec![],
        instance: None,
    };
    let mut rx = request(&app, old, operation.clone(), CancellationToken::new());
    assert_eq!(
        wait(&mut app, &mut rx).unwrap_err().kind,
        StorageErrorKind::OwnerMissing
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut rx = request(&app, owner, operation, cancel);
    assert_eq!(
        wait(&mut app, &mut rx).unwrap_err().kind,
        StorageErrorKind::Cancelled
    );
}
#[test]
fn actual_write_errors_return_through_original_oneshot() {
    let dir = tempfile::tempdir().unwrap();
    let storage =
        open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "scalar=1").unwrap();
    let mut app = app();
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, storage);
    let mut rx = request(
        &app,
        owner,
        StorageOperation::SetConfig {
            path: "scalar.child".into(),
            value: 1.into(),
        },
        CancellationToken::new(),
    );
    assert_eq!(
        wait(&mut app, &mut rx).unwrap_err().kind,
        StorageErrorKind::ConfigTypeMismatch
    );
}
#[cfg(unix)]
#[test]
fn shutdown_waits_for_real_worker_join_and_delivers_committed_result() {
    use std::{ffi::CString, io::Write, os::unix::ffi::OsStrExt};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    let cfg = dir.path().join("cfg");
    let storage = open_storage(&db, &cfg, "").unwrap();
    std::fs::remove_file(&cfg).unwrap();
    let name = CString::new(cfg.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut app = app();
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, storage);
    let mut rx = request(
        &app,
        owner,
        StorageOperation::SetConfig {
            path: "saved".into(),
            value: true.into(),
        },
        CancellationToken::new(),
    );
    app.tick();
    app.tick();
    let mut writer = std::fs::OpenOptions::new().write(true).open(&cfg).unwrap();
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        writer.write_all(b"before=1\n").unwrap();
    });
    let start = Instant::now();
    drop(app);
    assert!(start.elapsed() >= Duration::from_millis(30));
    release.join().unwrap();
    assert_eq!(rx.try_recv().unwrap().unwrap(), StorageValue::Unit);
    let settings = std::fs::read_to_string(cfg).unwrap();
    assert!(settings.contains("saved = true"));
    assert!(settings.contains("before = 1"));
}
#[test]
#[should_panic(expected = "already installed")]
fn duplicate_installation_is_rejected() {
    app().add_plugin(StoragePlugin::new());
}
#[test]
#[should_panic(expected = "requires AsyncRuntimePlugin")]
fn missing_async_facilities_are_rejected() {
    App::new().add_plugin(StoragePlugin::new().with_schedule("update"));
}

#[test]
fn worker_appends_duplicate_payloads_with_distinct_sequences_and_original_content() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    let mut app = app();
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, storage);
    let record = HistoryRecord {
        source: "agent:caller/原始地址".into(),
        turn: Some("turn".into()),
        kind: "message".into(),
        content: "caller supplied content".into(),
        payload: serde_json::json!({"type":"assistant", "content":"different payload content"}),
        created_at_ms: Some(0),
    };
    for _ in 0..2 {
        let mut rx = request(
            &app,
            owner,
            StorageOperation::AppendHistory {
                records: vec![record.clone()],
            },
            CancellationToken::new(),
        );
        assert_eq!(wait(&mut app, &mut rx).unwrap(), StorageValue::Unit);
    }
    let mut rx = request(
        &app,
        owner,
        StorageOperation::ReadHistoryEntries,
        CancellationToken::new(),
    );
    assert_eq!(
        wait(&mut app, &mut rx).unwrap(),
        StorageValue::HistoryEntries(vec![
            StoredHistoryRecord {
                sequence: 1,
                record: record.clone()
            },
            StoredHistoryRecord {
                sequence: 2,
                record
            },
        ])
    );
}
