use crate::*;

#[test]
fn busy_close_keeps_connection_for_retry_and_blocks_business_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let settings = dir.path().join("setting.toml");
    let storage = open_storage(&path, &settings, "").unwrap();
    let clone = storage.clone();
    let mut statement = std::ptr::null_mut();
    {
        let guard = storage.database.shared.connection.lock().unwrap();
        let connection = guard.as_ref().unwrap();
        // A deliberately unfinalized SQLite statement makes sqlite3_close return BUSY.
        // It is confined to this test and finalized before retrying the same connection.
        let result = unsafe {
            rusqlite::ffi::sqlite3_prepare_v2(
                connection.handle(),
                c"SELECT 1".as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
    }
    assert_eq!(close_storage(&storage).unwrap_err().kind, K::CloseFailed);
    assert!(storage.database.shared.connection.lock().unwrap().is_some());
    assert!(!clone.is_open());
    assert_eq!(
        execute_storage(&clone, StorageOperation::ReadConfig)
            .unwrap_err()
            .kind,
        K::Closed
    );
    assert_eq!(
        open_storage(&path, &settings, "").err().unwrap().kind,
        K::Closed
    );
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_finalize(statement) },
        rusqlite::ffi::SQLITE_OK
    );
    close_storage(&clone).unwrap();
    assert!(storage.database.shared.connection.lock().unwrap().is_none());
    close_storage(&storage).unwrap();
}

#[test]
fn close_waits_for_active_management_gate_and_releases_temp_data() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(
        &dir.path().join("state.sqlite"),
        &dir.path().join("setting.toml"),
        "",
    )
    .unwrap();
    execute_storage(
        &storage,
        StorageOperation::CreateBlock {
            name: "temporary".into(),
            fields: vec!["messages".into()],
            instance: Some("vm".into()),
        },
    )
    .unwrap();
    let guard = storage.database.shared.connection.lock().unwrap();
    let closer = storage.clone();
    let (started, start) = std::sync::mpsc::channel();
    let (done, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = close_storage(&closer);
        done.send(result).unwrap();
    });
    start.recv().unwrap();
    assert!(matches!(
        completion.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(guard);
    completion.recv().unwrap().unwrap();
    worker.join().unwrap();
    assert!(storage.database.shared.temporary.lock().unwrap().is_empty());
    assert!(!storage.is_open());
}
