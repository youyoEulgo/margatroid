use std::sync::{Arc, Mutex};
use storage_core::*;
fn report(id: Option<&str>) -> RuntimeLogInput {
    RuntimeLogInput {
        report_id: id.map(str::to_owned),
        reporter: "client:webui/test".into(),
        target: "ui.decode".into(),
        agent: Some("agent:test/coder".into()),
        record: AgentLogRecord {
            created_at_ms: Some(123),
            level: AgentLogLevel::Error,
            source: AgentLogSource::Host,
            stage: "frontend".into(),
            message: "完整错误报告".into(),
            details_json: Some(
                serde_json::json!({"code":"InvalidFrame","raw_frame":"{broken","fields":[{"name":"type","value":"agent.sync"}]}),
            ),
            context_json: Some(
                serde_json::json!({"request_id":"req-1","client":"client:webui/test"}),
            ),
            origin: Some("workbench.ts:812".into()),
            raw_error: Some("decode failed: {broken".into()),
            traceback: Some("Error: decode\n at handleFrame (workbench.ts:812)".into()),
        },
    }
}
fn range() -> RecordRange {
    RecordRange {
        after_sequence: None,
        before_sequence: None,
        limit: 100,
    }
}
#[test]
fn runtime_reports_are_durable_complete_and_probe_runs_after_commit_outside_the_database_lock() {
    let root = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(None::<RuntimeLogStore>));
    let inspect = shared.clone();
    let commits = Arc::new(Mutex::new(Vec::new()));
    let recorded = commits.clone();
    let store = open_runtime_logs(
        &root.path().join("runtime.sqlite"),
        RuntimeLogProbe::new(move |event| {
            let store = inspect.lock().unwrap().as_ref().unwrap().clone();
            let page = store.read(range()).unwrap();
            assert!(page.records.iter().any(|row| row == &event));
            recorded.lock().unwrap().push(event);
        }),
    )
    .unwrap();
    *shared.lock().unwrap() = Some(store.clone());
    let saved = store.append(report(Some("browser-report-1"))).unwrap();
    assert_eq!(saved.input, report(Some("browser-report-1")));
    assert_eq!(commits.lock().unwrap().len(), 1);
    let repeated = store.append(report(Some("browser-report-1"))).unwrap();
    assert_eq!(saved, repeated);
    assert_eq!(
        commits.lock().unwrap().len(),
        1,
        "retry emitted a fake new commit"
    );
    *shared.lock().unwrap() = None;
    drop(store);
    let reopened = open_runtime_logs(
        &root.path().join("runtime.sqlite"),
        RuntimeLogProbe::new(|_| {}),
    )
    .unwrap();
    assert_eq!(reopened.read(range()).unwrap().records, vec![saved]);
}
#[test]
fn invalid_reports_do_not_commit_or_publish() {
    let root = tempfile::tempdir().unwrap();
    let commits = Arc::new(Mutex::new(0));
    let count = commits.clone();
    let store = open_runtime_logs(
        &root.path().join("runtime.sqlite"),
        RuntimeLogProbe::new(move |_| *count.lock().unwrap() += 1),
    )
    .unwrap();
    let mut invalid = report(None);
    invalid.record.context_json = Some(serde_json::json!([]));
    assert!(store.append(invalid).is_err());
    assert!(store.read(range()).unwrap().records.is_empty());
    assert_eq!(*commits.lock().unwrap(), 0);
}
#[test]
fn existing_incompatible_log_database_is_not_migrated_or_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("runtime.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE user_data(value TEXT);INSERT INTO user_data VALUES('preserve');",
    )
    .unwrap();
    drop(db);
    assert!(open_runtime_logs(&path, RuntimeLogProbe::new(|_| {})).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM user_data", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "preserve"
    );
}

#[test]
fn concurrent_log_writes_publish_in_database_sequence_order() {
    let root = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let saved = seen.clone();
    let store = open_runtime_logs(
        &root.path().join("runtime.sqlite"),
        RuntimeLogProbe::new(move |record| saved.lock().unwrap().push(record.sequence)),
    )
    .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let store = store.clone();
            scope.spawn(move || {
                for _ in 0..20 {
                    store.append(report(None)).unwrap();
                }
            });
        }
    });
    assert_eq!(*seen.lock().unwrap(), (1..=80).collect::<Vec<u64>>());
}
