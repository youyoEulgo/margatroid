use agent_driver_core::*;
use std::{
    fs,
    path::Path,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

fn write(root: &Path, path: &str, data: impl AsRef<[u8]>) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, data).unwrap();
}
async fn matching(
    receiver: &mut mpsc::UnboundedReceiver<DriverUiState>,
    predicate: impl Fn(&DriverUiState) -> bool,
) -> DriverUiState {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = receiver.recv().await.unwrap();
            if predicate(&value) {
                return value;
            }
        }
    })
    .await
    .expect("UI monitor did not publish expected state")
}
#[test]
fn read_complete_resources_has_stable_content_identity_and_strict_budget() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("driver.toml");
    assert!(
        read_driver_ui(&manifest, DriverUiReadLimits::default())
            .unwrap()
            .is_none()
    );
    write(root.path(), "ui/index.html", b"<html>hello</html>");
    write(root.path(), "ui/assets/app.js", b"export const value=1");
    write(root.path(), "ui/assets/image.png", [0, 1, 2, 255]);
    let first = read_driver_ui(&manifest, DriverUiReadLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(first.entry, "index.html");
    assert_eq!(
        first
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["assets/app.js", "assets/image.png", "index.html"]
    );
    assert_eq!(first.files[1].data, [0, 1, 2, 255]);
    assert_eq!(first.revision.len(), 71);
    write(root.path(), "ui/assets/app.js", b"export const value=1");
    assert_eq!(
        read_driver_ui(&manifest, DriverUiReadLimits::default())
            .unwrap()
            .unwrap()
            .revision,
        first.revision
    );
    write(root.path(), "ui/assets/app.js", b"export const value=2");
    assert_ne!(
        read_driver_ui(&manifest, DriverUiReadLimits::default())
            .unwrap()
            .unwrap()
            .revision,
        first.revision
    );
    assert_eq!(
        read_driver_ui(&manifest, DriverUiReadLimits { raw_bytes: 1 }),
        Err(DriverUiError::TooLarge)
    );
    let entry = root.path().join("ui/index.html");
    assert!(entry.starts_with(root.path()));
    fs::remove_file(entry).unwrap();
    assert_eq!(
        read_driver_ui(&manifest, DriverUiReadLimits::default()),
        Err(DriverUiError::EntryMissing)
    );
}
#[cfg(unix)]
#[test]
fn ui_files_and_root_cannot_escape_the_driver_directory() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(root.path(), "ui/index.html", "safe");
    write(outside.path(), "secret", "private");
    std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("ui/leak")).unwrap();
    assert_eq!(
        read_driver_ui(
            &root.path().join("driver.toml"),
            DriverUiReadLimits::default()
        ),
        Err(DriverUiError::PathInvalid)
    );
    let link_root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), link_root.path().join("ui")).unwrap();
    assert_eq!(
        read_driver_ui(
            &link_root.path().join("driver.toml"),
            DriverUiReadLimits::default()
        ),
        Err(DriverUiError::PathInvalid)
    );
}
#[tokio::test]
async fn monitor_observes_creation_edits_deletion_errors_and_recovery_without_queries() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("driver");
    fs::create_dir(&directory).unwrap();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let (watch, monitor) = open_driver_ui_watch(
        directory.join("driver.toml"),
        Arc::new(move |state| {
            sender.send(state).unwrap();
        }),
    );
    let task = tokio::spawn(run_driver_ui_monitor(monitor));
    matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Ready(None))
    })
    .await;
    write(&directory, "ui/index.html", "first");
    let first = matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Ready(Some(_)))
    })
    .await;
    let DriverUiState::Ready(Some(first)) = first else {
        unreachable!()
    };
    write(&directory, "ui/index.html", "first");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), receiver.recv())
            .await
            .is_err()
    );
    write(&directory, "ui/assets/app.js", "second");
    matching(
        &mut receiver,
        |value| matches!(value,DriverUiState::Ready(Some(bundle)) if bundle.files.len()==2),
    )
    .await;
    let entry = directory.join("ui/index.html");
    assert!(entry.starts_with(root.path()));
    fs::remove_file(&entry).unwrap();
    matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Failed(DriverUiError::EntryMissing))
    })
    .await;
    write(&directory, "ui/index.html", "first");
    matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Ready(Some(_)))
    })
    .await;
    let ui = directory.join("ui");
    assert!(ui.starts_with(root.path()));
    fs::remove_dir_all(&ui).unwrap();
    matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Ready(None))
    })
    .await;
    write(&directory, "ui/index.html", "first");
    let restored = matching(&mut receiver, |value| {
        matches!(value, DriverUiState::Ready(Some(_)))
    })
    .await;
    let DriverUiState::Ready(Some(restored)) = restored else {
        unreachable!()
    };
    assert_eq!(restored.revision, first.revision);
    watch.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn cancel_waits_for_the_real_publication_worker_to_return() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "ui/index.html", "ready");
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = entered.clone();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let blocked = gate.clone();
    let (watch, monitor) = open_driver_ui_watch(
        root.path().join("driver.toml"),
        Arc::new(move |_| {
            observed.notify_one();
            let (lock, condition) = &*blocked;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = condition.wait(released).unwrap();
            }
        }),
    );
    let task = tokio::spawn(run_driver_ui_monitor(monitor));
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    watch.cancel();
    assert!(!task.is_finished());
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
