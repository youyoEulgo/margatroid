use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use storage_core::*;

fn storage(dir: &Path) -> AgentStorage {
    open_storage(&dir.join("state.sqlite"), &dir.join("settings.toml"), "").unwrap()
}
fn root(dir: &Path) -> PathBuf {
    dir.join("state.sqlite.blocks")
}
fn create(s: &AgentStorage, name: &str, instance: Option<&str>) {
    execute_storage(
        s,
        StorageOperation::CreateBlock {
            name: name.into(),
            fields: vec!["context".into(), "pending".into()],
            instance: instance.map(str::to_owned),
        },
    )
    .unwrap();
}
fn selector(name: &str, field: &str, instance: Option<&str>) -> MessageSelector {
    MessageSelector {
        block: name.into(),
        field: field.into(),
        instance: instance.map(str::to_owned),
        selection: MessageSelection::All,
    }
}
fn write(s: &AgentStorage, name: &str, field: &str, value: Value, instance: Option<&str>) {
    execute_storage(
        s,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![value])],
            target: selector(name, field, instance),
        },
    )
    .unwrap();
}
fn read(s: &AgentStorage, name: &str, field: &str, instance: Option<&str>) -> StorageValue {
    execute_storage(
        s,
        StorageOperation::ReadMessages {
            selectors: vec![selector(name, field, instance)],
        },
    )
    .unwrap()
}
#[test]
fn snapshot_ancestry_restore_renames_old_timeline_and_preserves_every_child() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    write(
        &s,
        "session",
        "context",
        json!({"type":"assistant","nested":[null,true,"你好"]}),
        None,
    );
    write(&s, "session", "pending", message(json!("first")), None);
    let initial = read_block_snapshots(&s, "session", Some("vm")).unwrap();
    assert_eq!(initial["snapshots"], json!([]));
    assert!(initial["timeline"]["upstream_id"].is_null());
    let path = root(d.path()).join(initial["timeline"]["file_name"].as_str().unwrap());
    let canonical = path.clone();
    snapshot_block(&s, "session", Some("vm")).unwrap();
    assert_eq!(
        read_block_snapshots(&s, "session", None).unwrap()["timeline"]["file_name"],
        initial["timeline"]["file_name"]
    );
    write(&s, "session", "context", message(json!(2)), None);
    write(&s, "session", "pending", message(json!("second")), None);
    snapshot_block(&s, "session", None).unwrap();
    write(&s, "session", "context", message(json!(3)), None);
    write(&s, "session", "pending", message(json!("third")), None);
    let before = fs::read(&path).unwrap();
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&path).unwrap().ino()
    };
    restore_block(&s, "session", None, 1).unwrap();
    let map = read_block_snapshots(&s, "session", None).unwrap();
    assert_eq!(map["timeline"]["upstream_id"], 1);
    assert_eq!(map["snapshots"].as_array().unwrap().len(), 3);
    assert!(map["snapshots"][0]["upstream_id"].is_null());
    assert_eq!(map["snapshots"][1]["upstream_id"], 1);
    assert_eq!(map["snapshots"][2]["upstream_id"], 2);
    let archived = root(d.path()).join(map["snapshots"][2]["file_name"].as_str().unwrap());
    assert_eq!(fs::read(&archived).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&archived).unwrap().ino(), inode);
    }
    assert_eq!(path, canonical);
    assert_eq!(
        read(&s, "session", "pending", None),
        StorageValue::Messages(vec![message(json!("first"))])
    );
    assert_eq!(
        read(&s, "session", "context", None),
        StorageValue::Messages(vec![
            json!({"type":"assistant","nested":[null,true,"你好"]})
        ])
    );
    snapshot_block(&s, "session", None).unwrap();
    let map = read_block_snapshots(&s, "session", None).unwrap();
    assert_eq!(map["snapshots"][3]["upstream_id"], 1);
    drop(s);
    let reopened = storage(d.path());
    assert_eq!(
        read_block_snapshots(&reopened, "session", None).unwrap(),
        map
    );
}
#[test]
fn identifiers_are_block_local_and_temporary_files_are_removed_only_for_their_owner() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    create(&s, "another", None);
    create(&s, "temp", Some("vm-a"));
    create(&s, "temp", Some("vm-b"));
    for (name, ns) in [
        ("session", None),
        ("another", None),
        ("temp", Some("vm-a")),
        ("temp", Some("vm-b")),
    ] {
        snapshot_block(&s, name, ns).unwrap();
        assert_eq!(
            read_block_snapshots(&s, name, ns).unwrap()["snapshots"][0]["snapshot_id"],
            1
        );
    }
    snapshot_block(&s, "session", None).unwrap();
    assert!(restore_block(&s, "another", None, 2).is_err());
    let old = read_block_snapshots(&s, "temp", Some("vm-a")).unwrap();
    execute_storage(
        &s,
        StorageOperation::DropTemporaryBlocks {
            instance: "vm-a".into(),
        },
    )
    .unwrap();
    for item in old["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .chain(std::iter::once(&old["timeline"]))
    {
        assert!(item["file_name"].is_null());
    }
    assert!(read_block_snapshots(&s, "temp", Some("vm-a")).is_err());
    assert!(read_block_snapshots(&s, "temp", Some("vm-b")).is_ok());
    assert!(read_block_snapshots(&s, "session", None).is_ok());
    drop(s);
    let s = storage(d.path());
    assert!(read_block_snapshots(&s, "temp", Some("vm-b")).is_err());
    assert!(read_block_snapshots(&s, "session", None).is_ok());
}
#[test]
fn child_tables_have_natural_rows_and_wal_commits_are_included_in_snapshot() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    let map = read_block_snapshots(&s, "session", None).unwrap();
    let path = root(d.path()).join(map["timeline"]["file_name"].as_str().unwrap());
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;INSERT INTO f0(position,role,content,payload) VALUES(0,'user','in wal','{}');").unwrap();
    snapshot_block(&s, "session", None).unwrap();
    let map = read_block_snapshots(&s, "session", None).unwrap();
    let snap =
        Connection::open(root(d.path()).join(map["snapshots"][0]["file_name"].as_str().unwrap()))
            .unwrap();
    assert_eq!(
        snap.query_row("SELECT count(*) FROM f0", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let tables: Vec<String> = snap
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tables, ["f0", "f1"]);
    drop(db);
    drop(snap);
    restore_block(&s, "session", None, 1).unwrap();
    assert_eq!(
        read(&s, "session", "context", None),
        StorageValue::Messages(vec![json!({"type":"user","content":"in wal"})])
    );
}
#[test]
fn restore_recovers_same_intent_before_rename_after_rename_and_after_publish() {
    for phase in 0..3 {
        let d = tempfile::tempdir().unwrap();
        let s = storage(d.path());
        create(&s, "session", None);
        write(&s, "session", "context", message(json!(1)), None);
        snapshot_block(&s, "session", None).unwrap();
        write(&s, "session", "context", message(json!(2)), None);
        snapshot_block(&s, "session", None).unwrap();
        write(&s, "session", "context", message(json!(3)), None);
        let map = read_block_snapshots(&s, "session", None).unwrap();
        let work = root(d.path()).join(map["timeline"]["file_name"].as_str().unwrap());
        let archive = work.parent().unwrap().join("snapshot_3.sqlite");
        let target = root(d.path()).join(map["snapshots"][0]["file_name"].as_str().unwrap());
        drop(s);
        let db = Connection::open(d.path().join("state.sqlite")).unwrap();
        db.execute("UPDATE block_snapshots SET restore_intent=?1 WHERE kind='timeline'",[json!({"archive_id":3,"target_id":1,"target_fields":map["snapshots"][0]["fields_json"]}).to_string()]).unwrap();
        drop(db);
        if phase >= 1 {
            fs::rename(&work, &archive).unwrap();
        }
        if phase >= 2 {
            fs::copy(&target, &work).unwrap();
        }
        let s = storage(d.path());
        assert_eq!(
            read(&s, "session", "context", None),
            StorageValue::Messages(vec![message(json!(1))])
        );
        let map = read_block_snapshots(&s, "session", None).unwrap();
        assert_eq!(map["snapshots"].as_array().unwrap().len(), 3);
        assert_eq!(map["snapshots"][2]["upstream_id"], 2);
        assert_eq!(map["timeline"]["upstream_id"], 1);
        drop(s);
        let s = storage(d.path());
        assert_eq!(read_block_snapshots(&s, "session", None).unwrap(), map);
    }
}
#[test]
fn failed_final_mapping_commit_seals_storage_then_recovers_without_extra_archive() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    write(&s, "session", "context", message(json!(1)), None);
    snapshot_block(&s, "session", None).unwrap();
    write(&s, "session", "context", message(json!(2)), None);
    let db = Connection::open(d.path().join("state.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_archive BEFORE INSERT ON block_snapshots WHEN NEW.snapshot_id=2 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(restore_block(&s, "session", None, 1).is_err());
    assert_eq!(
        read_block_snapshots(&s, "session", None).unwrap_err().kind,
        StorageErrorKind::CleanupFailed
    );
    drop(s);
    db.execute_batch("DROP TRIGGER fail_archive;").unwrap();
    drop(db);
    let s = storage(d.path());
    let map = read_block_snapshots(&s, "session", None).unwrap();
    assert_eq!(map["snapshots"].as_array().unwrap().len(), 2);
    assert_eq!(map["timeline"]["upstream_id"], 1);
    assert_eq!(
        read(&s, "session", "context", None),
        StorageValue::Messages(vec![message(json!(1))])
    );
}
#[test]
fn invalid_restore_does_not_change_work_or_consume_a_snapshot_identifier() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    write(&s, "session", "context", message(json!(1)), None);
    let before = read_block_snapshots(&s, "session", None).unwrap();
    for id in [0, 1, u64::MAX] {
        assert!(restore_block(&s, "session", None, id).is_err());
        assert_eq!(read_block_snapshots(&s, "session", None).unwrap(), before);
    }
    for name in ["../session", "session.context", ""] {
        assert!(snapshot_block(&s, name, None).is_err());
    }
    snapshot_block(&s, "session", None).unwrap();
    assert_eq!(
        read_block_snapshots(&s, "session", None).unwrap()["snapshots"][0]["snapshot_id"],
        1
    );
}

#[test]
fn publication_recovery_removes_only_proven_unpublished_files_and_keeps_committed_snapshots() {
    fn marker(
        root: &Path,
        name: &str,
        id: i64,
        file: &str,
        fields: &Value,
        upstream: Value,
    ) -> PathBuf {
        let encoded: String = name.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let marker = root.join(format!("publish__{encoded}_{id}.json"));
        fs::write(&marker,json!({"name":name,"instance":"","id":id,"file":file,"fields":fields,"upstream":upstream,"intent":null}).to_string()).unwrap();
        marker
    }
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    snapshot_block(&s, "session", None).unwrap();
    let map = read_block_snapshots(&s, "session", None).unwrap();
    let root = root(d.path());
    let timeline = root.join(map["timeline"]["file_name"].as_str().unwrap());
    let file = timeline.parent().unwrap().join("snapshot_2.sqlite");
    let relative = file
        .strip_prefix(&root)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    fs::copy(&timeline, &file).unwrap();
    marker(
        &root,
        "session",
        2,
        &relative,
        &map["timeline"]["fields_json"],
        json!(1),
    );
    let committed = &map["snapshots"][0];
    marker(
        &root,
        "session",
        1,
        committed["file_name"].as_str().unwrap(),
        &committed["fields_json"],
        Value::Null,
    );
    let external = root.join("external-data.sqlite");
    fs::write(&external, b"not ours").unwrap();
    drop(s);
    let s = storage(d.path());
    assert!(!file.exists());
    assert_eq!(fs::read(external).unwrap(), b"not ours");
    assert_eq!(read_block_snapshots(&s, "session", None).unwrap(), map);
    snapshot_block(&s, "session", None).unwrap();
    assert_eq!(
        read_block_snapshots(&s, "session", None).unwrap()["snapshots"][1]["snapshot_id"],
        2
    );
    // Simulate CREATE crashing after the owned file is created but before mapping commit.
    let orphan = root.join("orphan").join("timeline.sqlite");
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"partial sqlite").unwrap();
    marker(
        &root,
        "orphan",
        0,
        orphan.strip_prefix(&root).unwrap().to_str().unwrap(),
        &json!({"context":"f0","pending":"f1"}),
        Value::Null,
    );
    drop(s);
    let s = storage(d.path());
    assert!(!orphan.exists());
    create(&s, "orphan", None);
}
#[test]
fn missing_timeline_is_rejected_without_creating_an_empty_database() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    snapshot_block(&s, "session", None).unwrap();
    let map = read_block_snapshots(&s, "session", None).unwrap();
    let path = root(d.path()).join(map["timeline"]["file_name"].as_str().unwrap());
    fs::remove_file(&path).unwrap();
    assert!(restore_block(&s, "session", None, 1).is_err());
    assert!(!path.exists());
}

#[test]
fn snapshot_crash_child() {
    let Some(dir) = std::env::var_os("STORAGE_SNAPSHOT_CRASH_DIR") else {
        return;
    };
    let path = Path::new(&dir);
    let phase = std::env::var("STORAGE_SNAPSHOT_CRASH_PHASE").unwrap();
    let s = storage(path);
    create(&s, "session", None);
    write(&s, "session", "context", message(json!(1)), None);
    snapshot_block(&s, "session", None).unwrap();
    write(&s, "session", "context", message(json!(2)), None);
    snapshot_block(&s, "session", None).unwrap();
    write(&s, "session", "context", message(json!(3)), None);
    let map = read_block_snapshots(&s, "session", None).unwrap();
    let work = root(path).join(map["timeline"]["file_name"].as_str().unwrap());
    let archive = work.parent().unwrap().join("snapshot_3.sqlite");
    let db = Connection::open(path.join("state.sqlite")).unwrap();
    if phase == "failed_commit" {
        db.execute_batch("CREATE TRIGGER fail_archive BEFORE INSERT ON block_snapshots WHEN NEW.snapshot_id=3 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(restore_block(&s, "session", None, 1).is_err());
    } else {
        db.execute("UPDATE block_snapshots SET restore_intent=?1 WHERE kind='timeline'",[json!({"archive_id":3,"target_id":1,"target_fields":map["snapshots"][0]["fields_json"]}).to_string()]).unwrap();
        if phase == "renamed" {
            fs::rename(&work, &archive).unwrap();
        }
    }
    fs::File::create(path.join("ready"))
        .unwrap()
        .sync_all()
        .unwrap();
    // Parent kills the actual process: SQLite connections and AgentStorage never run Drop.
    loop {
        std::thread::park();
    }
}

#[test]
fn killed_process_restore_intents_recover_without_duplicate_archive() {
    use std::process::{Command, Stdio};
    for phase in ["intent", "renamed", "failed_commit"] {
        let d = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "snapshot_crash_child", "--nocapture"])
            .env("STORAGE_SNAPSHOT_CRASH_DIR", d.path())
            .env("STORAGE_SNAPSHOT_CRASH_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        while !d.path().join("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("crash helper exited early: {status}");
            }
            if start.elapsed() > std::time::Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("helper timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        if phase == "failed_commit" {
            Connection::open(d.path().join("state.sqlite"))
                .unwrap()
                .execute_batch("DROP TRIGGER fail_archive;")
                .unwrap();
        }
        let s = storage(d.path());
        assert_eq!(
            read(&s, "session", "context", None),
            StorageValue::Messages(vec![message(json!(1))])
        );
        let map = read_block_snapshots(&s, "session", None).unwrap();
        assert_eq!(map["snapshots"].as_array().unwrap().len(), 3);
        assert_eq!(map["snapshots"][2]["upstream_id"], 2);
        assert_eq!(map["timeline"]["upstream_id"], 1);
    }
}

#[test]
fn incomplete_marker_recovers_and_temporary_snapshots_never_touch_disk() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    let before = fs::read(d.path().join("state.sqlite")).unwrap();
    create(&s, "temp", Some("crashed"));
    write(&s, "temp", "context", message(json!(1)), Some("crashed"));
    snapshot_block(&s, "temp", Some("crashed")).unwrap();
    write(&s, "temp", "context", message(json!(2)), Some("crashed"));
    restore_block(&s, "temp", Some("crashed"), 1).unwrap();
    assert_eq!(
        read(&s, "temp", "context", Some("crashed")),
        StorageValue::Messages(vec![message(json!(1))])
    );
    assert!(!root(d.path()).exists());
    assert_eq!(before, fs::read(d.path().join("state.sqlite")).unwrap());
    assert_eq!(
        Connection::open(d.path().join("state.sqlite"))
            .unwrap()
            .query_row("SELECT count(*) FROM block_snapshots", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    fs::create_dir(root(d.path())).unwrap();
    let pending = root(d.path()).join("publish__deadbeef_0.pending");
    fs::write(&pending, b"{partial").unwrap();
    drop(s);
    let s = storage(d.path());
    assert!(!pending.exists());
    assert!(read_block_snapshots(&s, "temp", Some("crashed")).is_err());
}

#[test]
fn unregistered_restore_destination_is_not_mistaken_for_current_timeline() {
    let d = tempfile::tempdir().unwrap();
    let s = storage(d.path());
    create(&s, "session", None);
    write(&s, "session", "context", message(json!(1)), None);
    snapshot_block(&s, "session", None).unwrap();
    write(&s, "session", "context", message(json!(2)), None);
    let before = read_block_snapshots(&s, "session", None).unwrap();
    let work = root(d.path()).join(before["timeline"]["file_name"].as_str().unwrap());
    let collision = work.parent().unwrap().join("snapshot_2.sqlite");
    fs::write(&collision, b"unowned file").unwrap();
    assert!(restore_block(&s, "session", None, 1).is_err());
    assert_eq!(fs::read(collision).unwrap(), b"unowned file");
    assert_eq!(
        read(&s, "session", "context", None),
        StorageValue::Messages(vec![message(json!(2))])
    );
    assert_eq!(read_block_snapshots(&s, "session", None).unwrap(), before);
}

fn message(value: Value) -> Value {
    json!({"type":"user","content":value.to_string()})
}
