use core_plugin::World;
use margatroid_types::block_storage::*;
use margatroid_types::block_types::{BlockPath, InnerType};
use margatroid_types::lua_runtime_components::DriverLuaVmInstance;
use margatroid_types::message_types::{AgentMessage, TokenUsage, ToolCall};
use memory_plugin::{AgentHistoryStore, SqliteAgentDatabase};
use resource_id_plugin::ResourceID;
use tokio_util::sync::CancellationToken;

fn context(start: &str) -> BlockStorageContext {
    let mut world = World::new();
    BlockStorageContext {
        agent: ResourceID::parse("agent:local/test").unwrap(),
        instance: DriverLuaVmInstance {
            owner: world.spawn(),
            start_request_id: start.into(),
        },
    }
}
fn run(
    db: &SqliteAgentDatabase,
    context: &BlockStorageContext,
    operation: BlockStorageOperation,
) -> Result<StoredBlockResult, BlockStorageError> {
    db.execute(BlockStorageRequest {
        context: context.clone(),
        operation,
        cancel: CancellationToken::new(),
    })
}
fn create(
    db: &SqliteAgentDatabase,
    c: &BlockStorageContext,
    name: &str,
    temporary: bool,
) -> Result<StoredBlockResult, BlockStorageError> {
    run(
        db,
        c,
        BlockStorageOperation::Create(BlockSchema {
            name: name.into(),
            fields: vec![
                ("messages".into(), InnerType::Message),
                ("tools".into(), InnerType::Tool),
                ("sandboxes".into(), InnerType::Sandbox),
            ],
            temporary,
        }),
    )
}
fn selector(name: &str, field: &str) -> BlockSelector {
    BlockSelector {
        path: BlockPath {
            block_id: name.into(),
            inner_id: field.into(),
        },
        range: BlockRange::All,
    }
}
fn get(db: &SqliteAgentDatabase, c: &BlockStorageContext, s: BlockSelector) -> StoredBlockInner {
    match run(
        db,
        c,
        BlockStorageOperation::Get {
            expressions: vec![BlockExpression::Sources(vec![BlockSource::Selector(s)])],
        },
    )
    .unwrap()
    {
        StoredBlockResult::Arrays(mut values) => values.remove(0),
        _ => panic!(),
    }
}
fn inject(
    db: &SqliteAgentDatabase,
    c: &BlockStorageContext,
    target: BlockSelector,
    value: StoredBlockInner,
) {
    assert_eq!(
        run(
            db,
            c,
            BlockStorageOperation::Inject {
                source: BlockExpression::Sources(vec![BlockSource::Values(value)]),
                target
            }
        )
        .unwrap(),
        StoredBlockResult::Unit
    );
}
fn full_message() -> StoredBlockInner {
    StoredBlockInner::Message(vec![AgentMessage::Inject {
        messages: vec![
            AgentMessage::System {
                content: "system".into(),
            },
            AgentMessage::Assistant {
                reasoning: Some("reason".into()),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call".into(),
                    tool_name: "bash".into(),
                    arguments: "{}".into(),
                }],
                usage: Some(TokenUsage {
                    input_tokens: 12,
                    output_tokens: 3,
                    cache_hit_tokens: 0,
                }),
                stop_reason: None,
            },
            AgentMessage::ToolCall {
                resource_id: Some(ResourceID::parse("tool:local/bash").unwrap()),
                tool_call_id: "call".into(),
                tool_name: "bash".into(),
                content: "output".into(),
                failed: true,
            },
        ],
    }])
}

#[test]
fn persistent_restart_preserves_full_message_ids_and_history_is_separate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.sqlite");
    let c = context("one");
    let db = SqliteAgentDatabase::open(&path).unwrap();
    assert!(matches!(
        create(&db, &c, "session", false).unwrap(),
        StoredBlockResult::BlockOpened(BlockOpened { created: true, .. })
    ));
    let messages = full_message();
    inject(&db, &c, selector("session", "messages"), messages.clone());
    let tools = StoredBlockInner::Tool(vec![
        ResourceID::parse("tool:local/bash").unwrap(),
        ResourceID::parse("tool:local/review:v1").unwrap(),
    ]);
    let sandboxes =
        StoredBlockInner::Sandbox(vec![ResourceID::parse("sandbox:local/write").unwrap()]);
    inject(&db, &c, selector("session", "tools"), tools.clone());
    inject(&db, &c, selector("session", "sandboxes"), sandboxes.clone());
    assert!(db.list().unwrap().is_empty());
    drop(db);
    let db = SqliteAgentDatabase::open(&path).unwrap();
    let c = context("two");
    assert!(matches!(
        create(&db, &c, "session", false).unwrap(),
        StoredBlockResult::BlockOpened(BlockOpened { created: false, .. })
    ));
    assert_eq!(get(&db, &c, selector("session", "messages")), messages);
    assert_eq!(get(&db, &c, selector("session", "tools")), tools);
    assert_eq!(get(&db, &c, selector("session", "sandboxes")), sandboxes);
    let conn = rusqlite::Connection::open(path).unwrap();
    let json: String = conn
        .query_row(
            "SELECT value_json FROM block_fields WHERE name='tools'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(json.contains("tool:local/bash:latest"));
    assert!(!json.contains("definition"));
    assert!(!json.contains("policy"));
}

#[test]
fn temporary_isolation_idempotence_and_visible_lifecycle_conflicts() {
    let db = SqliteAgentDatabase::open(":memory:").unwrap();
    let a = context("one");
    let b = context("two");
    create(&db, &a, "entry", true).unwrap();
    inject(&db, &a, selector("entry", "messages"), full_message());
    assert!(matches!(
        create(&db, &a, "entry", true).unwrap(),
        StoredBlockResult::BlockOpened(BlockOpened { created: false, .. })
    ));
    assert_eq!(
        create(&db, &a, "entry", false),
        Err(BlockStorageError::LifecycleMismatch("entry".into()))
    );
    create(&db, &b, "entry", true).unwrap();
    assert!(get(&db, &b, selector("entry", "messages")).is_empty());
    assert!(!get(&db, &a, selector("entry", "messages")).is_empty());
    create(&db, &a, "shared", false).unwrap();
    assert_eq!(
        create(&db, &b, "shared", true),
        Err(BlockStorageError::LifecycleMismatch("shared".into()))
    );
}

#[test]
fn schema_range_type_and_database_failure_leave_fields_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite");
    let db = SqliteAgentDatabase::open(&path).unwrap();
    let c = context("one");
    create(&db, &c, "b", false).unwrap();
    let before = full_message();
    inject(&db, &c, selector("b", "messages"), before.clone());
    assert!(matches!(
        run(
            &db,
            &c,
            BlockStorageOperation::Create(BlockSchema {
                name: "b".into(),
                fields: vec![("messages".into(), InnerType::Tool)],
                temporary: false
            })
        ),
        Err(BlockStorageError::SchemaMismatch(_))
    ));
    let mut target = selector("b", "messages");
    target.range = BlockRange::Single(i64::MAX);
    assert_eq!(
        run(
            &db,
            &c,
            BlockStorageOperation::Inject {
                source: BlockExpression::Empty,
                target
            }
        ),
        Err(BlockStorageError::InvalidRange)
    );
    assert_eq!(
        run(
            &db,
            &c,
            BlockStorageOperation::Inject {
                source: BlockExpression::Sources(vec![BlockSource::Values(
                    StoredBlockInner::Tool(vec![])
                )]),
                target: selector("b", "messages")
            }
        ),
        Err(BlockStorageError::TypeMismatch)
    );
    rusqlite::Connection::open(path).unwrap().execute_batch("CREATE TRIGGER fail_replace BEFORE UPDATE ON block_fields BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
    assert!(matches!(
        run(
            &db,
            &c,
            BlockStorageOperation::Inject {
                source: BlockExpression::Empty,
                target: selector("b", "messages")
            }
        ),
        Err(BlockStorageError::Io(_))
    ));
    assert_eq!(get(&db, &c, selector("b", "messages")), before);
}

#[test]
fn self_source_reads_all_sources_before_replacement_and_empty_is_typed() {
    let db = SqliteAgentDatabase::open(":memory:").unwrap();
    let c = context("one");
    create(&db, &c, "b", false).unwrap();
    let message = full_message();
    inject(&db, &c, selector("b", "messages"), message.clone());
    run(
        &db,
        &c,
        BlockStorageOperation::Inject {
            source: BlockExpression::Sources(vec![
                BlockSource::Selector(selector("b", "messages")),
                BlockSource::Selector(selector("b", "messages")),
            ]),
            target: selector("b", "messages"),
        },
    )
    .unwrap();
    let mut expected = message.clone();
    expected.append(message).unwrap();
    assert_eq!(get(&db, &c, selector("b", "messages")), expected);
    run(
        &db,
        &c,
        BlockStorageOperation::Inject {
            source: BlockExpression::Empty,
            target: selector("b", "messages"),
        },
    )
    .unwrap();
    assert_eq!(
        get(&db, &c, selector("b", "messages")),
        StoredBlockInner::Message(vec![])
    );
}

#[test]
fn cancellation_before_commit_is_unchanged_after_commit_does_not_flip_success() {
    let db = SqliteAgentDatabase::open(":memory:").unwrap();
    let c = context("one");
    create(&db, &c, "b", false).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        db.execute(BlockStorageRequest {
            context: c.clone(),
            operation: BlockStorageOperation::Inject {
                source: BlockExpression::Sources(vec![BlockSource::Values(full_message())]),
                target: selector("b", "messages")
            },
            cancel
        }),
        Err(BlockStorageError::Cancelled)
    );
    assert!(get(&db, &c, selector("b", "messages")).is_empty());
    let cancel = CancellationToken::new();
    let result = db.execute(BlockStorageRequest {
        context: c.clone(),
        operation: BlockStorageOperation::Inject {
            source: BlockExpression::Sources(vec![BlockSource::Values(full_message())]),
            target: selector("b", "messages"),
        },
        cancel: cancel.clone(),
    });
    cancel.cancel();
    assert_eq!(result, Ok(StoredBlockResult::Unit));
    assert_eq!(get(&db, &c, selector("b", "messages")), full_message());
}

#[test]
fn malformed_database_schema_rejected_without_resetting_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE block_definitions(id INTEGER PRIMARY KEY, agent TEXT, instance TEXT, name TEXT, temporary INTEGER, schema_json TEXT); INSERT INTO block_definitions VALUES(1,'agent','instance','name',1,'keep');").unwrap();
    assert!(matches!(
        SqliteAgentDatabase::open(&path),
        Err(BlockStorageError::SchemaMismatch(_))
    ));
    let value: String = conn
        .query_row(
            "SELECT schema_json FROM block_definitions WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "keep");
}

#[test]
fn partial_uniqueness_bad_primary_key_and_missing_foreign_key_are_rejected() {
    for (case, definitions, fields, extra) in [
        ("partial", "id INTEGER PRIMARY KEY,agent TEXT NOT NULL,instance TEXT NOT NULL,name TEXT NOT NULL,temporary INTEGER NOT NULL,schema_json TEXT NOT NULL", "definition_id INTEGER NOT NULL REFERENCES block_definitions(id),name TEXT NOT NULL,inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id,name)", "CREATE UNIQUE INDEX partial_def ON block_definitions(agent,instance,name) WHERE temporary=1;"),
        ("id", "id TEXT PRIMARY KEY,agent TEXT NOT NULL,instance TEXT NOT NULL,name TEXT NOT NULL,temporary INTEGER NOT NULL,schema_json TEXT NOT NULL,UNIQUE(agent,instance,name)", "definition_id INTEGER NOT NULL REFERENCES block_definitions(id),name TEXT NOT NULL,inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id,name)", ""),
        ("fk", "id INTEGER PRIMARY KEY,agent TEXT NOT NULL,instance TEXT NOT NULL,name TEXT NOT NULL,temporary INTEGER NOT NULL,schema_json TEXT NOT NULL,UNIQUE(agent,instance,name)", "definition_id INTEGER NOT NULL,name TEXT NOT NULL,inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id,name)", ""),
        ("nullable", "id INTEGER PRIMARY KEY,agent TEXT,instance TEXT NOT NULL,name TEXT NOT NULL,temporary INTEGER NOT NULL,schema_json TEXT NOT NULL,UNIQUE(agent,instance,name)", "definition_id INTEGER NOT NULL REFERENCES block_definitions(id),name TEXT NOT NULL,inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id,name)", ""),
    ] {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("bad.sqlite"); let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE block_definitions({definitions}); CREATE TABLE block_fields({fields}); {extra} INSERT INTO block_definitions VALUES(1,'agent','instance','name',1,'keep');")).unwrap();
        assert!(matches!(SqliteAgentDatabase::open(&path), Err(BlockStorageError::SchemaMismatch(_))), "{case}");
        let value: String = conn.query_row("SELECT schema_json FROM block_definitions", [], |r| r.get(0)).unwrap(); assert_eq!(value, "keep", "{case}");
    }
}

#[test]
fn persistent_create_cannot_strand_another_instances_temporary_block() {
    let db = SqliteAgentDatabase::open(":memory:").unwrap();
    let a = context("a");
    let b = context("b");
    create(&db, &a, "entry", true).unwrap();
    inject(&db, &a, selector("entry", "messages"), full_message());
    assert_eq!(
        create(&db, &b, "entry", false),
        Err(BlockStorageError::LifecycleMismatch("entry".into()))
    );
    assert_eq!(get(&db, &a, selector("entry", "messages")), full_message());
}

#[test]
fn async_handle_and_history_view_share_database_and_worker_commit_truth() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let db = SqliteAgentDatabase::open(":memory:").unwrap();
    let c = context("one");
    let handle = AgentDatabaseHandle::new(db.clone());
    handle
        .history()
        .append(memory_plugin::HistoryRecord {
            created_at_ms: 0,
            key: "one".into(),
            source: "test".into(),
            turn: None,
            kind: "test".into(),
            content: "content".into(),
            payload: serde_json::json!({}),
        })
        .unwrap();
    assert_eq!(db.list().unwrap().len(), 1);
    let cancel = CancellationToken::new();
    let result = runtime.block_on(handle.block_storage().execute(BlockStorageRequest {
        context: c.clone(),
        operation: BlockStorageOperation::Create(BlockSchema {
            name: "b".into(),
            fields: vec![("messages".into(), InnerType::Message)],
            temporary: false,
        }),
        cancel: cancel.clone(),
    }));
    cancel.cancel();
    assert!(matches!(
        result,
        Ok(StoredBlockResult::BlockOpened(BlockOpened {
            created: true,
            ..
        }))
    ));
    assert_eq!(
        get(&db, &c, selector("b", "messages")),
        StoredBlockInner::Message(vec![])
    );
}

#[test]
fn concurrent_multi_get_keeps_one_snapshot_even_across_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    let db = SqliteAgentDatabase::open(&path).unwrap();
    let c = context("one");
    create(&db, &c, "b", false).unwrap();
    let writer = SqliteAgentDatabase::open(&path).unwrap();
    let wc = c.clone();
    let join = std::thread::spawn(move || {
        for n in 0..100 {
            inject(
                &writer,
                &wc,
                selector("b", "messages"),
                StoredBlockInner::Message(vec![AgentMessage::User {
                    content: n.to_string(),
                }]),
            );
        }
    });
    for _ in 0..100 {
        let expression =
            BlockExpression::Sources(vec![BlockSource::Selector(selector("b", "messages"))]);
        let result = run(
            &db,
            &c,
            BlockStorageOperation::Get {
                expressions: vec![expression.clone(), expression],
            },
        )
        .unwrap();
        match result {
            StoredBlockResult::Arrays(values) => assert_eq!(values[0], values[1]),
            _ => panic!(),
        }
    }
    join.join().unwrap();
}
