use rusqlite::Connection;
use serde_json::{json, Value};
use storage_core::*;

fn open(dir: &std::path::Path) -> AgentStorage {
    open_storage(&dir.join("state.sqlite"), &dir.join("setting.toml"), "").unwrap()
}
fn selector() -> MessageSelector {
    MessageSelector {
        block: "session".into(),
        field: "messages".into(),
        instance: None,
        selection: MessageSelection::All,
    }
}
#[test]
fn normalized_columns_round_trip_every_role_without_redundant_payload() {
    let d = tempfile::tempdir().unwrap();
    let storage = open(d.path());
    let messages = vec![
        json!({"type":"system","content":"instructions","meta":"keep"}),
        json!({"type":"user","content":"question","data":[{"media_type":"image/png","data":"AA=="}]}),
        json!({"type":"assistant","reasoning":"think once","content":"answer","data":{"custom":true},"tool_calls":[{"id":"call","tool_name":"read","arguments":"{}"}],"usage":{"input_tokens":1}}),
        json!({"type":"tool_call","tool_call_id":"call","tool_name":"read","content":"result","data":[],"failed":false}),
        json!({"type":"inject","messages":[{"type":"user","content":"nested"}],"meta":1}),
        json!({"type":"error","message":"failure","request_id":"request"}),
    ];
    let saved = append_message_history(&storage, &messages, Some("turn")).unwrap();
    for (i, (saved, original)) in saved.iter().zip(&messages).enumerate() {
        let mut expected = original.clone();
        expected["sequence"] = json!(i + 1);
        assert_eq!(*saved, expected);
    }
    execute_storage(
        &storage,
        StorageOperation::CreateBlock {
            name: "session".into(),
            fields: vec!["messages".into()],
            instance: None,
        },
    )
    .unwrap();
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(saved.clone())],
            target: selector(),
        },
    )
    .unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector()]
            }
        )
        .unwrap(),
        StorageValue::Messages(saved.clone())
    );
    let db = Connection::open(d.path().join("state.sqlite")).unwrap();
    let raw: (String, String, String, String) = db
        .query_row(
            "SELECT role,content,tool_call,payload FROM agent_messages WHERE sequence=3",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(raw.0, "assistant");
    assert_eq!(
        serde_json::from_str::<Value>(&raw.1).unwrap(),
        json!({"reasoning":"think once","content":"answer"})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&raw.2).unwrap(),
        messages[2]["tool_calls"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(&raw.3).unwrap(),
        json!({"usage":{"input_tokens":1}})
    );
    let block =
        Connection::open(d.path().join("state.sqlite.blocks/session/timeline.sqlite")).unwrap();
    let rows: Vec<(Option<i64>, String)> = block
        .prepare("SELECT sequence,payload FROM f0 ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[2].0, Some(3));
    assert_eq!(rows[2].1, raw.3);
    let StorageValue::HistoryEntries(history) =
        execute_storage(&storage, StorageOperation::ReadAgentMessages).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        history
            .into_iter()
            .map(|r| r.record.payload)
            .collect::<Vec<_>>(),
        saved
    );
    drop(block);
    drop(db);
    drop(storage);
    let reopened = open(d.path());
    assert_eq!(
        execute_storage(
            &reopened,
            StorageOperation::ReadMessages {
                selectors: vec![selector()]
            }
        )
        .unwrap(),
        StorageValue::Messages(saved)
    );
}
#[test]
fn only_real_history_sequences_are_accepted_and_array_positions_are_independent() {
    let d = tempfile::tempdir().unwrap();
    let storage = open(d.path());
    let messages = append_message_history(
        &storage,
        &[
            json!({"type":"user","content":"a"}),
            json!({"type":"user","content":"b"}),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        append_message_history(&storage, &messages, None).unwrap(),
        messages
    );
    let StorageValue::HistoryEntries(entries) =
        execute_storage(&storage, StorageOperation::ReadAgentMessages).unwrap()
    else {
        panic!()
    };
    assert_eq!(entries.len(), 2);
    for message in [
        json!({"type":"user","content":"forged","sequence":1}),
        json!({"type":"user","content":"a","sequence":99}),
    ] {
        assert_eq!(
            append_message_history(&storage, &[message], None)
                .unwrap_err()
                .kind,
            StorageErrorKind::InvalidRecord
        );
    }
    execute_storage(
        &storage,
        StorageOperation::CreateBlock {
            name: "session".into(),
            fields: vec!["messages".into()],
            instance: None,
        },
    )
    .unwrap();
    let ordered = vec![
        messages[1].clone(),
        json!({"type":"user","content":"not yet history"}),
        messages[0].clone(),
    ];
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(ordered.clone())],
            target: selector(),
        },
    )
    .unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![selector()]
            }
        )
        .unwrap(),
        StorageValue::Messages(ordered)
    );
    let db =
        Connection::open(d.path().join("state.sqlite.blocks/session/timeline.sqlite")).unwrap();
    let seq: Vec<Option<i64>> = db
        .prepare("SELECT sequence FROM f0 ORDER BY position")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(seq, [Some(2), None, Some(1)]);
    for bad in [
        json!({"type":"assistant","content":"x","replay":{}}),
        json!({"type":"user","content":"x","images":[]}),
        json!({"type":"unknown","content":"x"}),
    ] {
        assert!(append_message_history(&storage, &[bad], None).is_err());
    }
}

#[test]
fn temp_create_opens_existing_content_and_payload_duplicates_are_rejected() {
    let d = tempfile::tempdir().unwrap();
    let storage = open(d.path());
    let create = || StorageOperation::CreateBlock {
        name: "entry".into(),
        fields: vec!["pending".into()],
        instance: Some("vm".into()),
    };
    execute_storage(&storage, create()).unwrap();
    let target = MessageSelector {
        block: "entry".into(),
        field: "pending".into(),
        instance: Some("vm".into()),
        selection: MessageSelection::All,
    };
    let value = json!({"type":"user","content":"keep"});
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![value.clone()])],
            target: target.clone(),
        },
    )
    .unwrap();
    execute_storage(&storage, create()).unwrap();
    assert_eq!(
        execute_storage(
            &storage,
            StorageOperation::ReadMessages {
                selectors: vec![target]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![value])
    );
    assert!(!d.path().join("state.sqlite.blocks").exists());
    append_message_history(
        &storage,
        &[json!({"type":"assistant","content":"answer","reasoning":"thought"})],
        None,
    )
    .unwrap();
    let db = Connection::open(d.path().join("state.sqlite")).unwrap();
    for key in [
        "content",
        "reasoning",
        "data",
        "replay",
        "images",
        "sequence",
        "type",
        "tool_calls",
    ] {
        db.execute(
            "UPDATE agent_messages SET payload=?1",
            [json!({key:"duplicate"}).to_string()],
        )
        .unwrap();
        assert_eq!(
            execute_storage(&storage, StorageOperation::ReadAgentMessages)
                .unwrap_err()
                .kind,
            StorageErrorKind::SchemaMismatch
        );
    }
}
