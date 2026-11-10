use serde_json::json;
use storage_core::*;

#[test]
fn mixed_message_sources_keep_literal_and_stored_order_without_losing_structure() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    create(&storage, "a", None);
    create(&storage, "b", Some("current"));
    set(&storage, selector("a", None), 1);
    set(&storage, selector("b", Some("current")), 2);
    let literal = json!({"type":"system","content":"prompt","metadata":[null,true]});
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessageSources {
                sources: vec![
                    MessageSource::Stored(selector("a", Some("current"))),
                    MessageSource::Literal(vec![literal.clone()]),
                    MessageSource::Stored(selector("b", Some("current"))),
                    MessageSource::Literal(vec![]),
                    MessageSource::Stored(selector("a", Some("current")))
                ]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![
            message(json!(1)),
            literal,
            message(json!(2)),
            message(json!(1))
        ])
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessageSources {
                sources: vec![
                    MessageSource::Literal(vec![message(json!("prefix"))]),
                    MessageSource::Stored(selector("missing", Some("current")))
                ]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessageSources { sources: vec![] }
        )
        .unwrap(),
        StorageValue::Messages(vec![])
    );
}
#[test]
fn mixed_literal_and_stored_sources_use_one_snapshot_during_concurrent_storage_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    create(&storage, "a", None);
    create(&storage, "b", Some("current"));
    set(&storage, selector("a", None), 0);
    set(&storage, selector("b", Some("current")), 0);
    let writer = storage.clone();
    let worker = std::thread::spawn(move || {
        for n in 1..100 {
            set(&writer, selector("a", None), n);
        }
    });
    for _ in 0..100 {
        let StorageValue::Messages(values) = execute_storage(
            &storage,
            StorageOperation::ReadMessageSources {
                sources: vec![
                    MessageSource::Stored(selector("a", Some("current"))),
                    MessageSource::Literal(vec![message(json!("prompt"))]),
                    MessageSource::Stored(selector("a", Some("current"))),
                ],
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], values[2]);
        assert_eq!(values[1], message(json!("prompt")));
    }
    worker.join().unwrap();
}

fn selector(block: &str, instance: Option<&str>) -> MessageSelector {
    MessageSelector {
        block: block.into(),
        field: "messages".into(),
        instance: instance.map(Into::into),
        selection: MessageSelection::All,
    }
}
fn create(storage: &AgentStorage, name: &str, instance: Option<&str>) {
    execute_storage(
        storage,
        StorageOperation::CreateBlock {
            name: name.into(),
            fields: vec!["messages".into()],
            instance: instance.map(Into::into),
        },
    )
    .unwrap();
}
fn set(storage: &AgentStorage, target: MessageSelector, value: i64) {
    execute_storage(
        storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![message(json!(value))])],
            target,
        },
    )
    .unwrap();
}
#[test]
fn current_vm_selector_falls_back_to_persistent_without_exposing_other_vm_temporary_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open_storage(&dir.path().join("db"), &dir.path().join("cfg"), "").unwrap();
    create(&storage, "persistent", None);
    create(&storage, "temp", Some("current-vm"));
    create(&storage, "temp", Some("other-vm"));
    create(&storage, "other-only", Some("other-vm"));
    set(&storage, selector("persistent", None), 1);
    set(&storage, selector("temp", Some("current-vm")), 2);
    set(&storage, selector("temp", Some("other-vm")), 3);
    set(&storage, selector("other-only", Some("other-vm")), 4);
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![
                    selector("persistent", Some("current-vm")),
                    selector("temp", Some("current-vm"))
                ]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![message(json!(1)), message(json!(2))])
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector("other-only", Some("current-vm"))]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector("temp", None)]
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![
                MessageSource::Stored(selector("persistent", Some("current-vm"))),
                MessageSource::Stored(selector("temp", Some("current-vm"))),
            ],
            target: selector("persistent", Some("current-vm")),
        },
    )
    .unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector("persistent", None)]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![message(json!(1)), message(json!(2))])
    );
    let before = execute_storage(
        &storage,
        StorageOperation::ReadMessages {
            selectors: vec![selector("persistent", None)],
        },
    )
    .unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReplaceMessages {
                sources: vec![MessageSource::Stored(selector(
                    "other-only",
                    Some("current-vm")
                ))],
                target: selector("persistent", Some("current-vm"))
            }
        )
        .unwrap_err()
        .kind,
        StorageErrorKind::BlockMissing
    );
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector("persistent", None)]
            }
        )
        .unwrap(),
        before
    );
}
#[test]
fn persistent_and_current_temp_selectors_share_one_snapshot_during_concurrent_storage_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let storage = open_storage(&path, &dir.path().join("cfg"), "").unwrap();
    create(&storage, "persistent", None);
    create(&storage, "temp", Some("current-vm"));
    set(&storage, selector("persistent", None), 0);
    set(&storage, selector("temp", Some("current-vm")), 0);
    let writer = storage.clone();
    let worker = std::thread::spawn(move || {
        for n in 1..100 {
            set(&writer, selector("persistent", None), n);
        }
    });
    for _ in 0..100 {
        let StorageValue::Messages(values) = execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![
                    selector("persistent", Some("current-vm")),
                    selector("persistent", Some("current-vm")),
                    selector("temp", Some("current-vm")),
                ],
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], values[1]);
        assert_eq!(values[2], message(json!(0)));
    }
    worker.join().unwrap();
}

fn message(value: serde_json::Value) -> serde_json::Value {
    json!({"type":"user","content":value.to_string()})
}
