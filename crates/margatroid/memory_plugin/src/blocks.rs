use std::collections::BTreeMap;

use margatroid_types::block_storage::*;
use margatroid_types::block_types::InnerType;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::database::SqliteAgentDatabase;

fn io(error: impl std::fmt::Display) -> BlockStorageError {
    BlockStorageError::Io(error.to_string())
}

fn instance(context: &BlockStorageContext) -> Result<String, BlockStorageError> {
    serde_json::to_string(&(
        context.instance.owner.index(),
        context.instance.owner.generation(),
        &context.instance.start_request_id,
    ))
    .map_err(io)
}

fn definition(
    connection: &Connection,
    context: &BlockStorageContext,
    name: &str,
) -> Result<Option<(i64, bool, String)>, BlockStorageError> {
    let mut statement = connection.prepare("SELECT id, temporary, schema_json FROM block_definitions WHERE agent=?1 AND name=?2 AND (instance='' OR instance=?3)").map_err(io)?;
    let values = statement
        .query_map(
            params![context.agent.to_string(), name, instance(context)?],
            |row| Ok((row.get(0)?, row.get::<_, bool>(1)?, row.get(2)?)),
        )
        .map_err(io)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(io)?;
    if values.len() > 1 {
        return Err(BlockStorageError::LifecycleMismatch(name.into()));
    }
    Ok(values.into_iter().next())
}

fn field(
    connection: &Connection,
    context: &BlockStorageContext,
    selector: &BlockSelector,
) -> Result<(i64, StoredBlockInner), BlockStorageError> {
    let (id, _, _) = definition(connection, context, &selector.path.block_id)?
        .ok_or_else(|| BlockStorageError::BlockMissing(selector.path.block_id.clone()))?;
    let stored: Option<(String, String)> = connection
        .query_row(
            "SELECT inner_type, value_json FROM block_fields WHERE definition_id=?1 AND name=?2",
            params![id, selector.path.inner_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(io)?;
    let (kind, json) =
        stored.ok_or_else(|| BlockStorageError::FieldMissing(selector.path.clone()))?;
    let value: StoredBlockInner = serde_json::from_str(&json).map_err(io)?;
    if kind != kind_name(value.inner_type()) {
        return Err(BlockStorageError::SchemaMismatch(
            selector.path.block_id.clone(),
        ));
    }
    Ok((id, value))
}

fn kind_name(kind: InnerType) -> &'static str {
    match kind {
        InnerType::Message => "MESSAGE",
        InnerType::Tool => "TOOL",
        InnerType::Sandbox => "SANDBOX",
    }
}

fn evaluate(
    connection: &Connection,
    context: &BlockStorageContext,
    expression: &BlockExpression,
    empty_kind: Option<InnerType>,
) -> Result<StoredBlockInner, BlockStorageError> {
    let sources = match expression {
        BlockExpression::Empty => {
            return empty_kind.map(StoredBlockInner::empty).ok_or_else(|| {
                BlockStorageError::InvalidRequest("untyped empty expression".into())
            })
        }
        BlockExpression::Sources(sources) => sources,
    };
    let mut result: Option<StoredBlockInner> = None;
    for source in sources {
        let value = match source {
            BlockSource::Values(value) => value.clone(),
            BlockSource::Selector(selector) => {
                let (_, value) = field(connection, context, selector)?;
                let (start, end) = selector.range.map(value.len(), false)?;
                value.slice(start, end)?
            }
        };
        if let Some(result) = &mut result {
            result.append(value)?;
        } else {
            result = Some(value);
        }
    }
    result.ok_or_else(|| {
        BlockStorageError::InvalidRequest("empty sources have no declared type".into())
    })
}

fn commit(transaction: &rusqlite::Transaction<'_>) -> Result<(), BlockStorageError> {
    transaction
        .execute_batch("COMMIT")
        .map_err(|error| match &error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::SystemIoFailure
                        | rusqlite::ErrorCode::DiskFull
                        | rusqlite::ErrorCode::CannotOpen
                ) =>
            {
                BlockStorageError::CommitUnknown(error.to_string())
            }
            _ => io(error),
        })
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use margatroid_types::history_components::{AgentHistoryStore, HistoryError, HistoryRecord};
    use margatroid_types::lua_runtime_components::DriverLuaVmInstance;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};

    fn deny_rollback(context: AuthContext<'_>) -> Authorization {
        if matches!(
            context.action,
            AuthAction::Transaction {
                operation: TransactionOperation::Rollback
            }
        ) {
            Authorization::Deny
        } else {
            Authorization::Allow
        }
    }

    #[test]
    fn rollback_denial_returns_cleanup_failed_and_retires_both_views() {
        let db = SqliteAgentDatabase::open(":memory:").unwrap();
        let mut world = core_plugin::World::new();
        let request = BlockStorageRequest {
            context: BlockStorageContext {
                agent: resource_id_plugin::ResourceID::parse("agent:local/test").unwrap(),
                instance: DriverLuaVmInstance {
                    owner: world.spawn(),
                    start_request_id: "one".into(),
                },
            },
            operation: BlockStorageOperation::Create(BlockSchema {
                name: String::new(),
                fields: vec![],
                temporary: false,
            }),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        db.connection
            .lock()
            .unwrap()
            .authorizer(Some(deny_rollback));
        assert!(matches!(
            db.execute(request.clone()),
            Err(BlockStorageError::CleanupFailed(_))
        ));
        assert!(!db.connection.lock().unwrap().is_autocommit());
        assert!(matches!(
            db.clone().execute(request),
            Err(BlockStorageError::CleanupFailed(_))
        ));
        assert!(matches!(db.list(), Err(HistoryError::CleanupFailed(_))));
        assert!(matches!(
            db.history_view().list(),
            Err(HistoryError::CleanupFailed(_))
        ));
    }

    #[test]
    fn history_rollback_denial_retires_block_view() {
        let db = SqliteAgentDatabase::open(":memory:").unwrap();
        let record = HistoryRecord {
            created_at_ms: 0,
            key: "key".into(),
            source: "test".into(),
            turn: None,
            kind: "test".into(),
            content: "same".into(),
            payload: serde_json::json!({}),
        };
        db.append_batch(vec![record.clone()]).unwrap();
        db.connection
            .lock()
            .unwrap()
            .authorizer(Some(deny_rollback));
        let mut conflict = record.clone();
        conflict.content = "different".into();
        assert!(matches!(
            db.append_batch(vec![
                HistoryRecord {
                    key: "new".into(),
                    ..record
                },
                conflict
            ]),
            Err(HistoryError::CleanupFailed(_))
        ));
        assert!(!db.connection.lock().unwrap().is_autocommit());
        assert!(db.retired.load(std::sync::atomic::Ordering::Acquire));
        assert!(matches!(db.list(), Err(HistoryError::CleanupFailed(_))));
    }
}

impl AgentBlockStorage for SqliteAgentDatabase {
    fn execute(
        &self,
        request: BlockStorageRequest,
    ) -> Result<StoredBlockResult, BlockStorageError> {
        request.check_cancelled()?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io("database connection mutex poisoned"))?;
        if self.retired.load(std::sync::atomic::Ordering::Acquire) {
            return Err(BlockStorageError::CleanupFailed(
                "database connection retired after uncertain transaction".into(),
            ));
        }
        request.check_cancelled()?;
        let behavior = if matches!(request.operation, BlockStorageOperation::Get { .. }) {
            TransactionBehavior::Deferred
        } else {
            TransactionBehavior::Immediate
        };
        let transaction = connection.transaction_with_behavior(behavior).map_err(io)?;
        let outcome = (|| {
            let result = match &request.operation {
                BlockStorageOperation::Create(schema) => {
                    if schema.name.is_empty() {
                        return Err(BlockStorageError::InvalidRequest(
                            "block name must not be empty".into(),
                        ));
                    }
                    let mut fields = BTreeMap::new();
                    for (name, kind) in &schema.fields {
                        if name.is_empty() || fields.insert(name.clone(), *kind).is_some() {
                            return Err(BlockStorageError::InvalidRequest(
                                "empty or duplicate field name".into(),
                            ));
                        }
                    }
                    let conflict: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM block_definitions WHERE agent=?1 AND name=?2 AND temporary<>?3)", params![request.context.agent.to_string(), schema.name, schema.temporary], |row| row.get(0)).map_err(io)?;
                    if conflict {
                        return Err(BlockStorageError::LifecycleMismatch(schema.name.clone()));
                    }
                    let canonical = serde_json::to_string(&fields).map_err(io)?;
                    let created = match definition(&transaction, &request.context, &schema.name)? {
                        Some((id, temporary, stored_schema)) => {
                            if temporary != schema.temporary {
                                return Err(BlockStorageError::LifecycleMismatch(
                                    schema.name.clone(),
                                ));
                            }
                            if stored_schema != canonical {
                                return Err(BlockStorageError::SchemaMismatch(schema.name.clone()));
                            }
                            let count: i64 = transaction
                                .query_row(
                                    "SELECT count(*) FROM block_fields WHERE definition_id=?1",
                                    [id],
                                    |row| row.get(0),
                                )
                                .map_err(io)?;
                            if usize::try_from(count).ok() != Some(fields.len()) {
                                return Err(BlockStorageError::SchemaMismatch(schema.name.clone()));
                            }
                            for (name, kind) in &fields {
                                let selector = BlockSelector {
                                    path: margatroid_types::block_types::BlockPath {
                                        block_id: schema.name.clone(),
                                        inner_id: name.clone(),
                                    },
                                    range: BlockRange::All,
                                };
                                let (actual_id, value) =
                                    field(&transaction, &request.context, &selector)?;
                                if actual_id != id || value.inner_type() != *kind {
                                    return Err(BlockStorageError::SchemaMismatch(
                                        schema.name.clone(),
                                    ));
                                }
                            }
                            false
                        }
                        None => {
                            let namespace = if schema.temporary {
                                instance(&request.context)?
                            } else {
                                String::new()
                            };
                            transaction.execute("INSERT INTO block_definitions(agent, instance, name, temporary, schema_json) VALUES(?1,?2,?3,?4,?5)", params![request.context.agent.to_string(), namespace, schema.name, schema.temporary, canonical]).map_err(io)?;
                            let id = transaction.last_insert_rowid();
                            for (name, kind) in fields {
                                let value = serde_json::to_string(&StoredBlockInner::empty(kind))
                                    .map_err(io)?;
                                transaction.execute("INSERT INTO block_fields(definition_id,name,inner_type,value_json) VALUES(?1,?2,?3,?4)", params![id,name,kind_name(kind),value]).map_err(io)?;
                            }
                            true
                        }
                    };
                    StoredBlockResult::BlockOpened(BlockOpened {
                        block_id: schema.name.clone(),
                        created,
                        temporary: schema.temporary,
                    })
                }
                BlockStorageOperation::Get { expressions } => {
                    let mut values = Vec::with_capacity(expressions.len());
                    for expression in expressions {
                        request.check_cancelled()?;
                        values.push(evaluate(&transaction, &request.context, expression, None)?);
                    }
                    StoredBlockResult::Arrays(values)
                }
                BlockStorageOperation::Inject { source, target } => {
                    let (id, mut value) = field(&transaction, &request.context, target)?;
                    let replacement = evaluate(
                        &transaction,
                        &request.context,
                        source,
                        Some(value.inner_type()),
                    )?;
                    let (start, end) = target.range.map(value.len(), true)?;
                    value.replace(start, end, replacement)?;
                    request.check_cancelled()?;
                    let json = serde_json::to_string(&value).map_err(io)?;
                    transaction
                    .execute(
                        "UPDATE block_fields SET value_json=?1 WHERE definition_id=?2 AND name=?3",
                        params![json, id, target.path.inner_id],
                    )
                    .map_err(io)?;
                    StoredBlockResult::Unit
                }
            };
            request.check_cancelled()?;
            commit(&transaction)?;
            Ok(result)
        })();
        match outcome {
            Ok(result) if transaction.is_autocommit() => Ok(result),
            Ok(_) => {
                self.retired
                    .store(true, std::sync::atomic::Ordering::Release);
                Err(BlockStorageError::CleanupFailed(
                    "commit returned without closing transaction".into(),
                ))
            }
            Err(error) => {
                if !transaction.is_autocommit() {
                    if let Err(rollback) = transaction.execute_batch("ROLLBACK") {
                        self.retired
                            .store(true, std::sync::atomic::Ordering::Release);
                        return Err(BlockStorageError::CleanupFailed(format!(
                            "{error}; rollback failed: {rollback}"
                        )));
                    }
                }
                if !transaction.is_autocommit() {
                    self.retired
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Err(BlockStorageError::CleanupFailed(format!(
                        "{error}; rollback did not close transaction"
                    )));
                }
                Err(error)
            }
        }
    }
}
