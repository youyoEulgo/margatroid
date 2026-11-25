use rusqlite::{params, Connection};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    time::Duration,
};
use workspace_core::*;

struct Fixture {
    _dir: tempfile::TempDir,
    db: WorkspaceDatabase,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for suffix in ["agents/normal", "agents/temporary", "agents/archived"] {
            fs::create_dir_all(dir.path().join(suffix)).unwrap();
        }
        let db = WorkspaceDatabase::create(
            dir.path().join("workspace.sqlite"),
            WorkspaceMetadata {
                address: "/demo".into(),
                alias: "Demo".into(),
                project_root: PathBuf::from("/srv/project"),
            },
        )
        .unwrap();
        Self { _dir: dir, db }
    }
    fn prepare(
        &self,
        op: &WorkspaceOperation,
        node: Option<&str>,
        driver: &str,
        args: serde_json::Value,
    ) -> WorkspaceAgent {
        self.db
            .prepare_agent(
                op,
                "/demo".into(),
                node.map(str::to_owned),
                Some("Coder".into()),
                driver.into(),
                args,
            )
            .unwrap()
    }
    fn register(&self, node: &str) -> WorkspaceAgent {
        let op = self.db.try_operation(node).unwrap();
        let rec = self.prepare(
            &op,
            Some(node),
            "/tools/driver/tool.toml",
            json!({"node_name":node}),
        );
        fs::create_dir(self.db.agent_directory(&rec).unwrap()).unwrap();
        self.db.commit_agent(&op, rec).unwrap()
    }
}
fn kind<T: std::fmt::Debug>(r: Result<T, WorkspaceError>) -> WorkspaceErrorKind {
    r.unwrap_err().kind
}
#[test]
fn snapshots_only_publish_confirmed_registration_and_actual_changes() {
    let f = Fixture::new();
    let mut watch = f.db.subscribe_changes();
    assert_eq!(watch.borrow_and_update().as_ref().unwrap().revision, 0);
    let op = f.db.try_operation("coder").unwrap();
    let rec = f.prepare(
        &op,
        Some("coder"),
        "/tools/driver/tool.toml",
        json!({"node_name":"coder"}),
    );
    assert!(!watch.has_changed().unwrap());
    assert!(f.db.list_agents().unwrap().is_empty());
    fs::create_dir(f.db.agent_directory(&rec).unwrap()).unwrap();
    let registered = f.db.commit_agent(&op, rec).unwrap();
    drop(op);
    assert!(watch.has_changed().unwrap());
    let initial = watch.borrow_and_update().clone().unwrap();
    assert_eq!(initial.revision, 1);
    assert_eq!(initial.agents, vec![registered.clone()]);
    let op = f.db.try_operation("coder").unwrap();
    let unchanged = f.prepare(
        &op,
        Some("coder"),
        "/tools/driver/tool.toml",
        json!({"node_name":"coder"}),
    );
    f.db.commit_agent(&op, unchanged).unwrap();
    drop(op);
    assert!(!watch.has_changed().unwrap());
    let op = f.db.try_operation("coder").unwrap();
    let changed =
        f.db.prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "/tools/driver/tool.toml".into(),
            json!({"node_name":"coder","path":"src"}),
        )
        .unwrap();
    assert_eq!(changed.alias, registered.alias);
    f.db.commit_agent(&op, changed).unwrap();
    drop(op);
    let second = watch.borrow_and_update().clone().unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(initial.agents[0], registered);
    assert_ne!(initial.agents, second.agents);
    f.db.agent_by_node("coder").unwrap();
    f.db.ensure_startable(&registered.id).unwrap();
    assert!(!watch.has_changed().unwrap());
}
#[test]
fn different_driver_parent_or_materials_cannot_take_over_existing_node() {
    let f = Fixture::new();
    let original = f.register("coder");
    for (parent, driver, expected) in [
        (
            "/demo",
            "/tools/other/tool.toml",
            WorkspaceErrorKind::DriverMismatch,
        ),
        (
            "/demo/parent",
            "/tools/driver/tool.toml",
            WorkspaceErrorKind::ParentMismatch,
        ),
    ] {
        let op = f.db.try_operation("coder").unwrap();
        assert_eq!(
            kind(f.db.prepare_agent(
                &op,
                parent.into(),
                Some("coder".into()),
                None,
                driver.into(),
                json!({})
            )),
            expected
        );
    }
    let op = f.db.try_operation("coder").unwrap();
    let mut candidate = f.prepare(
        &op,
        Some("coder"),
        "/tools/driver/tool.toml",
        json!({"changed":true}),
    );
    candidate.tool_path = "/tools/other/tool.toml".into();
    assert_eq!(
        kind(f.db.commit_agent(&op, candidate)),
        WorkspaceErrorKind::InvalidRequest
    );
    assert_eq!(f.db.agent_by_node("coder").unwrap(), original);
}
#[test]
fn operations_are_database_bound_and_single_preparation_is_enforced() {
    let a = Fixture::new();
    let b = Fixture::new();
    let op = a.db.try_operation("coder").unwrap();
    assert_eq!(
        kind(b.db.prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "/tools/a".into(),
            json!({})
        )),
        WorkspaceErrorKind::InvalidRequest
    );
    a.prepare(&op, Some("coder"), "/tools/a", json!({}));
    assert_eq!(
        kind(a.db.prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "/tools/a".into(),
            json!({})
        )),
        WorkspaceErrorKind::Busy
    );
    assert_eq!(kind(a.db.try_operation("coder")), WorkspaceErrorKind::Busy);
}
#[tokio::test]
async fn close_admission_and_idle_cover_retained_arc_operation() {
    let f = Fixture::new();
    let clone = f.db.clone();
    let op = Arc::new(f.db.try_operation("coder").unwrap());
    let last = op.clone();
    let mut changes = f.db.subscribe_changes();
    changes.borrow_and_update();
    f.db.close_admission();
    assert_eq!(
        kind(clone.try_operation("other")),
        WorkspaceErrorKind::Closed
    );
    assert_eq!(kind(f.db.close()), WorkspaceErrorKind::Busy);
    drop(op);
    assert!(
        tokio::time::timeout(Duration::from_millis(15), clone.wait_idle())
            .await
            .is_err()
    );
    let waiter = tokio::spawn(async move {
        clone.wait_idle().await;
        clone
    });
    tokio::task::yield_now().await;
    drop(last);
    let clone = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap();
    clone.close().unwrap();
    assert!(changes.has_changed().unwrap());
    assert!(changes.borrow_and_update().is_none());
    assert_eq!(kind(f.db.list_agents()), WorkspaceErrorKind::Closed);
    f.db.close().unwrap();
}
#[test]
fn close_gate_is_atomic_against_concurrent_node_admission() {
    let f = Fixture::new();
    let barrier = Arc::new(Barrier::new(9));
    let mut joins = vec![];
    for _ in 0..8 {
        let db = f.db.clone();
        let b = barrier.clone();
        joins.push(std::thread::spawn(move || {
            b.wait();
            db.try_operation("coder")
        }));
    }
    barrier.wait();
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| e.kind == WorkspaceErrorKind::Busy));
    drop(results);
    f.db.close().unwrap();
}
#[test]
fn already_admitted_create_can_settle_after_admission_closes() {
    let f = Fixture::new();
    let op = f.db.try_operation("coder").unwrap();
    f.db.close_admission();
    let rec = f.prepare(&op, Some("coder"), "/tools/a", json!({}));
    fs::create_dir(f.db.agent_directory(&rec).unwrap()).unwrap();
    f.db.commit_agent(&op, rec).unwrap();
    assert_eq!(kind(f.db.close()), WorkspaceErrorKind::Busy);
    drop(op);
    f.db.close().unwrap();
}
#[test]
fn temporary_agents_have_independent_records_and_runtime_nodes() {
    let f = Fixture::new();
    let mut ids = vec![];
    for _ in 0..2 {
        let key = f.db.temporary_operation_key().unwrap();
        let op = f.db.try_operation(&key).unwrap();
        let rec = f.prepare(&op, None, "/tools/a", json!({}));
        assert_eq!(rec.runtime_name, key);
        assert_eq!(rec.directory_name, rec.id);
        assert_eq!(rec.category, AgentCategory::Temporary);
        fs::create_dir(f.db.agent_directory(&rec).unwrap()).unwrap();
        let rec = f.db.commit_agent(&op, rec).unwrap();
        assert_eq!(f.db.agent_by_node(&key).unwrap(), rec);
        ids.push(rec.id);
    }
    assert_ne!(ids[0], ids[1]);
}
#[test]
fn unknown_existing_directory_is_never_silently_registered() {
    let f = Fixture::new();
    let dir = f.db.managed_directory().join("agents/normal/coder");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("keep"), "original").unwrap();
    let op = f.db.try_operation("coder").unwrap();
    assert_eq!(
        kind(f.db.prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "/tools/a".into(),
            json!({})
        )),
        WorkspaceErrorKind::UnregisteredDirectory
    );
    assert_eq!(fs::read_to_string(dir.join("keep")).unwrap(), "original");
    assert!(f.db.list_agents().unwrap().is_empty());
}
#[test]
fn rejected_commit_does_not_publish_or_overwrite_registration() {
    let f = Fixture::new();
    let mut watch = f.db.subscribe_changes();
    watch.borrow_and_update();
    let op = f.db.try_operation("coder").unwrap();
    let rec = f.prepare(&op, Some("coder"), "/tools/a", json!({}));
    fs::create_dir(f.db.agent_directory(&rec).unwrap()).unwrap();
    let external = Connection::open(f.db.path()).unwrap();
    external.execute_batch("CREATE TRIGGER reject_insert BEFORE INSERT ON agents BEGIN SELECT RAISE(ABORT,'injected registration failure'); END;").unwrap();
    assert_eq!(
        kind(f.db.commit_agent(&op, rec)),
        WorkspaceErrorKind::DatabaseFailed
    );
    assert!(!watch.has_changed().unwrap());
    assert!(f.db.list_agents().unwrap().is_empty());
}
#[test]
fn archival_preserves_all_data_and_historical_identity_and_releases_node() {
    let f = Fixture::new();
    let rec = f.register("coder");
    let source = f.db.agent_directory(&rec).unwrap();
    fs::create_dir(source.join("blocks")).unwrap();
    fs::write(source.join("blocks/data"), "content").unwrap();
    let op = f.db.try_operation("coder").unwrap();
    let archived = f.db.archive_agent(&op, &rec.id).unwrap();
    drop(op);
    assert_eq!(archived.category, AgentCategory::Archived);
    assert_ne!(archived.directory_name, rec.directory_name);
    assert_eq!(archived.previous_node_name.as_deref(), Some("coder"));
    assert_eq!(archived.previous_alias, rec.alias);
    assert_eq!(archived.alias, rec.alias);
    assert_eq!(archived.tool_arguments, rec.tool_arguments);
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(f.db.agent_directory(&archived).unwrap().join("blocks/data")).unwrap(),
        "content"
    );
    assert_eq!(
        kind(f.db.ensure_startable(&rec.id)),
        WorkspaceErrorKind::InvalidRequest
    );
    let second = f.register("coder");
    assert_ne!(second.id, rec.id);
    assert_eq!(f.db.list_agents().unwrap().len(), 2);
}
#[test]
fn archive_after_rename_sql_failure_keeps_intent_and_explicit_retry_completes() {
    let f = Fixture::new();
    let rec = f.register("coder");
    let mut watch = f.db.subscribe_changes();
    watch.borrow_and_update();
    let external = Connection::open(f.db.path()).unwrap();
    external.execute_batch("CREATE TRIGGER reject_archive BEFORE UPDATE OF category ON agents BEGIN SELECT RAISE(ABORT,'injected archive commit failure'); END;").unwrap();
    let op = f.db.try_operation("coder").unwrap();
    assert_eq!(
        kind(f.db.archive_agent(&op, &rec.id)),
        WorkspaceErrorKind::ArchivePending
    );
    assert!(!f.db.agent_directory(&rec).unwrap().exists());
    assert_eq!(
        kind(f.db.ensure_startable(&rec.id)),
        WorkspaceErrorKind::ArchivePending
    );
    assert!(!watch.has_changed().unwrap());
    assert_eq!(
        external
            .query_row("SELECT count(*) FROM archive_intents", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    external
        .execute_batch("DROP TRIGGER reject_archive")
        .unwrap();
    let archived = f.db.archive_agent(&op, &rec.id).unwrap();
    assert_eq!(archived.category, AgentCategory::Archived);
    assert!(watch.has_changed().unwrap());
    assert_eq!(
        external
            .query_row("SELECT count(*) FROM archive_intents", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
fn install_intent(f: &Fixture, rec: &WorkspaceAgent, destination: &str) {
    let conn = Connection::open(f.db.path()).unwrap();
    conn.execute(
        "INSERT INTO archive_intents VALUES(?1,'Normal',?2,?3)",
        params![rec.id, rec.directory_name, destination],
    )
    .unwrap();
}
#[test]
fn open_resumes_intents_before_and_after_directory_move() {
    for already_moved in [false, true] {
        let f = Fixture::new();
        let rec = f.register("coder");
        install_intent(&f, &rec, "new-archive-name");
        let source = f.db.agent_directory(&rec).unwrap();
        let target =
            f.db.managed_directory()
                .join("agents/archived/new-archive-name");
        fs::write(source.join("state"), "committed bytes").unwrap();
        if already_moved {
            fs::rename(&source, &target).unwrap();
        }
        f.db.close().unwrap();
        let reopened = WorkspaceDatabase::open(f.db.path()).unwrap();
        let archived = reopened.agent_by_id(&rec.id).unwrap();
        assert_eq!(archived.category, AgentCategory::Archived);
        assert_eq!(
            fs::read_to_string(target.join("state")).unwrap(),
            "committed bytes"
        );
        assert!(reopened
            .subscribe_changes()
            .borrow()
            .as_ref()
            .unwrap()
            .agents
            .iter()
            .all(|a| a.category == AgentCategory::Archived));
    }
}
#[test]
fn ambiguous_archive_recovery_refuses_to_guess_or_overwrite() {
    for both in [false, true] {
        let f = Fixture::new();
        let rec = f.register("coder");
        install_intent(&f, &rec, "destination");
        let source = f.db.agent_directory(&rec).unwrap();
        let dest = f.db.managed_directory().join("agents/archived/destination");
        if both {
            fs::create_dir(&dest).unwrap();
            fs::write(dest.join("keep"), "untouched").unwrap();
        } else {
            fs::remove_dir(&source).unwrap();
        }
        f.db.close().unwrap();
        assert_eq!(
            kind(WorkspaceDatabase::open(f.db.path())),
            WorkspaceErrorKind::RecoveryFailed
        );
        if both {
            assert_eq!(fs::read_to_string(dest.join("keep")).unwrap(), "untouched");
            assert!(source.is_dir());
        }
    }
}
#[test]
fn database_open_does_not_create_migrate_or_read_yaml() {
    let f = Fixture::new();
    let path = f.db.path().to_owned();
    let metadata = f.db.metadata().unwrap();
    f.db.close().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA user_version=99").unwrap();
    drop(c);
    assert_eq!(
        kind(WorkspaceDatabase::open(&path)),
        WorkspaceErrorKind::DatabaseFailed
    );
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        99
    );
    let missing = f.db.managed_directory().join("missing/workspace.sqlite");
    assert!(WorkspaceDatabase::open(&missing).is_err());
    assert!(!missing.exists());
    assert!(WorkspaceDatabase::open(f.db.managed_directory().join("workspace.yaml")).is_err());
    assert_eq!(
        kind(WorkspaceDatabase::create(&path, metadata)),
        WorkspaceErrorKind::DatabaseFailed
    );
}
#[test]
fn separate_open_is_not_shared_connection_and_clone_close_is_shared() {
    let f = Fixture::new();
    let clone = f.db.clone();
    let separate = WorkspaceDatabase::open(f.db.path()).unwrap();
    assert!(clone.same_connection(&f.db));
    assert!(!separate.same_connection(&f.db));
    clone.close().unwrap();
    assert!(!f.db.is_open());
    assert!(separate.is_open());
    separate.close().unwrap();
}
#[test]
fn invalid_node_parent_arguments_and_directory_paths_are_rejected() {
    let f = Fixture::new();
    for key in [
        "",
        "/coder",
        "a/b",
        ".",
        "..",
        "a:b",
        "a\\b",
        "~temporary-a",
        "\n",
    ] {
        assert!(f.db.try_operation(key).is_err(), "{key:?}");
    }
    for parent in [
        "/other",
        "/democracy",
        "/demo/../escape",
        "/demo//x",
        "/demo/x:",
        "/demo/",
    ] {
        let op = f.db.try_operation("coder").unwrap();
        assert!(
            f.db.prepare_agent(
                &op,
                parent.into(),
                Some("coder".into()),
                None,
                "/tools/a".into(),
                json!({})
            )
            .is_err(),
            "{parent}"
        );
    }
    let op = f.db.try_operation("coder").unwrap();
    assert!(f
        .db
        .prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "../driver".into(),
            json!({})
        )
        .is_err());
    assert!(f
        .db
        .prepare_agent(
            &op,
            "/demo".into(),
            Some("coder".into()),
            None,
            "/tools/a".into(),
            json!([])
        )
        .is_err());
}
#[cfg(unix)]
#[test]
fn managed_symlink_paths_never_escape_the_workspace() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    let rec = f.register("coder");
    let directory = f.db.agent_directory(&rec).unwrap();
    fs::remove_dir(&directory).unwrap();
    symlink(external.path(), &directory).unwrap();
    let op = f.db.try_operation("coder").unwrap();
    assert!(f.db.archive_agent(&op, &rec.id).is_err());
    assert!(external.path().is_dir());
    let link = f.db.managed_directory().join("linked");
    symlink(external.path(), &link).unwrap();
    assert!(WorkspaceDatabase::open(link.join("workspace.sqlite")).is_err());
}
#[test]
fn public_events_retain_original_reply_without_business_core_dependencies() {
    use core_plugin::World;
    use std::sync::Mutex;
    let f = Fixture::new();
    let mut world = World::new();
    let entity = world.spawn();
    world.insert_component(
        entity,
        Workspace::new(Path::new("/srv/project").into(), f.db),
    );
    let notifications = Arc::new(Mutex::new(vec![]));
    let target = notifications.clone();
    let context = WorkspaceUpdateContext::new(move |_, entity, kind| {
        target.lock().unwrap().push((entity, kind))
    });
    context.notify(&mut world, entity, WorkspaceUpdateKind::Installed);
    assert_eq!(notifications.lock().unwrap().len(), 1);
    let (sender, mut receiver) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let reply = Arc::new(Mutex::new(Some(sender)));
    let event = StartWorkspaceAgent::New {
        workspace: entity,
        tool_path: "/tools/a".into(),
        arguments: json!({"node_name":"coder"}),
        reply: reply.clone(),
    };
    world.emit_event(event);
    world.advance_event_frame();
    let event = world
        .event_reader::<StartWorkspaceAgent<String, String>>()
        .into_iter()
        .next()
        .unwrap();
    match event {
        StartWorkspaceAgent::New { reply: actual, .. } => {
            assert!(Arc::ptr_eq(actual, &reply));
            actual
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(Ok("result".into()))
                .unwrap();
        }
        _ => panic!("wrong variant"),
    };
    assert_eq!(receiver.try_recv().unwrap(), Ok("result".into()));
}

#[test]
fn schema_version_alone_does_not_make_an_incompatible_database_acceptable() {
    let f = Fixture::new();
    f.db.close().unwrap();
    let conn = Connection::open(f.db.path()).unwrap();
    conn.execute_batch("PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=replace(sql, 'node_name TEXT UNIQUE', 'node_name TEXT') WHERE name='agents'; PRAGMA writable_schema=OFF;").unwrap();
    drop(conn);
    assert_eq!(
        kind(WorkspaceDatabase::open(f.db.path())),
        WorkspaceErrorKind::DatabaseFailed
    );
}

#[test]
fn lifecycle_work_is_idle_only_after_startup_and_all_create_tokens_settle() {
    let pending = WorkspacePendingWork::default();
    assert!(!pending.is_idle());
    assert!(!pending.load_completed());
    let load = pending.begin_load().unwrap();
    assert!(pending.begin_load().is_none());
    let first = pending.begin_create();
    let second = pending.begin_create();
    drop(load);
    assert!(pending.load_completed());
    assert!(!pending.is_idle());
    drop(first);
    assert!(!pending.is_idle());
    drop(second);
    assert!(pending.is_idle());
}

#[test]
fn alias_transactions_publish_complete_snapshots_and_persist_without_rewriting_arguments() {
    let f = Fixture::new();
    let normal = f.register("coder");
    let mut watcher = f.db.subscribe_changes();
    let initial = watcher.borrow_and_update().clone().unwrap();
    assert_eq!(initial.metadata.alias, "Demo");
    f.db.rename_workspace_alias("  新项目  ".into()).unwrap();
    let changed = watcher.borrow_and_update().clone().unwrap();
    assert_eq!(changed.metadata.alias, "  新项目  ");
    assert_eq!(changed.agents, initial.agents);
    assert_eq!(changed.revision, initial.revision + 1);
    assert_eq!(f.db.metadata().unwrap(), changed.metadata);
    f.db.rename_workspace_alias("  新项目  ".into()).unwrap();
    assert!(!watcher.has_changed().unwrap());
    f.db.rename_agent_alias(&normal.id, Some("  名称  ".into()))
        .unwrap();
    let changed = watcher.borrow_and_update().clone().unwrap();
    assert_eq!(changed.metadata.alias, "  新项目  ");
    assert_eq!(changed.agents[0].alias.as_deref(), Some("  名称  "));
    assert_eq!(changed.agents[0].tool_arguments, normal.tool_arguments);
    f.db.rename_agent_alias(&normal.id, Some("  名称  ".into()))
        .unwrap();
    assert!(!watcher.has_changed().unwrap());
    let reopen = WorkspaceDatabase::open(f.db.path()).unwrap();
    assert_eq!(reopen.metadata().unwrap(), changed.metadata);
    assert_eq!(reopen.list_agents().unwrap(), changed.agents);
    f.db.rename_agent_alias(&normal.id, None).unwrap();
    assert!(watcher.borrow_and_update().as_ref().unwrap().agents[0]
        .alias
        .is_none());
    f.db.rename_agent_alias(&normal.id, None).unwrap();
    assert!(!watcher.has_changed().unwrap());
}

#[test]
fn alias_updates_cover_temporary_archived_and_missing_records() {
    let f = Fixture::new();
    let key = "~temporary-0123456789abcdef0123456789abcdef";
    let op = f.db.try_operation(key).unwrap();
    let temporary = f.prepare(&op, None, "/tools/driver/tool.toml", json!({}));
    fs::create_dir(f.db.agent_directory(&temporary).unwrap()).unwrap();
    f.db.commit_agent(&op, temporary.clone()).unwrap();
    drop(op);
    f.db.rename_agent_alias(&temporary.id, Some("临时".into()))
        .unwrap();
    let op = f.db.try_operation(key).unwrap();
    let archived = f.db.archive_agent(&op, &temporary.id).unwrap();
    drop(op);
    assert_eq!(archived.previous_alias.as_deref(), Some("临时"));
    f.db.rename_agent_alias(&temporary.id, Some("归档".into()))
        .unwrap();
    let archived = f.db.list_agents().unwrap().pop().unwrap();
    assert_eq!(archived.alias.as_deref(), Some("归档"));
    assert_eq!(archived.previous_alias.as_deref(), Some("临时"));
    assert_eq!(archived.category, AgentCategory::Archived);
    f.db.rename_agent_alias(&temporary.id, None).unwrap();
    assert_eq!(
        kind(f.db.rename_agent_alias("missing", Some("name".into()))),
        WorkspaceErrorKind::AgentRecordMissing
    );
    for invalid in ["", " ", "\t", "line\nbreak", "name\0"] {
        assert_eq!(
            kind(f.db.rename_workspace_alias(invalid.into())),
            WorkspaceErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(f.db.rename_agent_alias(&temporary.id, Some(invalid.into()))),
            WorkspaceErrorKind::InvalidRequest
        );
    }
    f.db.close().unwrap();
    assert_eq!(kind(f.db.metadata()), WorkspaceErrorKind::Closed);
    assert_eq!(
        kind(f.db.rename_workspace_alias("closed".into())),
        WorkspaceErrorKind::Closed
    );
    assert_eq!(
        kind(f.db.rename_agent_alias(&temporary.id, None)),
        WorkspaceErrorKind::Closed
    );
}

#[test]
fn alias_update_cannot_race_a_prepared_registration_or_archive() {
    let f = Fixture::new();
    let record = f.register("coder");
    let op = f.db.try_operation("coder").unwrap();
    let prepared = f.prepare(
        &op,
        Some("coder"),
        "/tools/driver/tool.toml",
        record.tool_arguments.clone(),
    );
    assert_eq!(
        kind(f.db.rename_agent_alias(&record.id, Some("lost".into()))),
        WorkspaceErrorKind::Busy
    );
    f.db.rename_workspace_alias("metadata during prepare".into())
        .unwrap();
    f.db.commit_agent(&op, prepared).unwrap();
    drop(op);
    assert_eq!(f.db.metadata().unwrap().alias, "metadata during prepare");
    f.db.rename_agent_alias(&record.id, Some("committed".into()))
        .unwrap();
    let op = f.db.try_operation("coder").unwrap();
    assert_eq!(
        kind(f.db.rename_agent_alias(&record.id, None)),
        WorkspaceErrorKind::Busy
    );
    f.db.archive_agent(&op, &record.id).unwrap();
    drop(op);
    let snapshot = f.db.subscribe_changes().borrow().clone().unwrap();
    assert_eq!(snapshot.metadata.alias, "metadata during prepare");
    assert_eq!(
        snapshot.agents[0].previous_alias.as_deref(),
        Some("committed")
    );
    assert_eq!(snapshot.agents[0].alias.as_deref(), Some("committed"));
}

#[test]
fn reused_record_keeps_authoritative_alias_but_records_actual_start_arguments() {
    let f = Fixture::new();
    let record = f.register("coder");
    for alias in [Some("Renamed authoritative alias".to_owned()), None] {
        f.db.rename_agent_alias(&record.id, alias.clone()).unwrap();
        let op = f.db.try_operation("coder").unwrap();
        let arguments = json!({"node_name":"coder","alias":"stale dispatched alias","path":"src","driver_setting":42});
        let candidate =
            f.db.prepare_agent(
                &op,
                "/demo".into(),
                Some("coder".into()),
                Some("stale dispatched alias".into()),
                record.tool_path.clone(),
                arguments.clone(),
            )
            .unwrap();
        assert_eq!(
            candidate.alias, alias,
            "existing record alias must not be overwritten by Driver arguments"
        );
        assert_eq!(candidate.tool_arguments, arguments);
        let committed = f.db.commit_agent(&op, candidate).unwrap();
        assert_eq!(committed.alias, alias);
        assert_eq!(
            committed.tool_arguments, arguments,
            "persist the actual invocation, not a rewritten reconstruction"
        );
        assert_eq!(committed.id, record.id);
    }
}
