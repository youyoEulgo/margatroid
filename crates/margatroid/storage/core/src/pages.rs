use crate::*;

fn read<T>(
    storage: &AgentStorage,
    operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let state = &storage.database.shared;
    let db = state
        .connection
        .lock()
        .map_err(|_| err(K::Closed, "database lock poisoned"))?;
    if state.retired.load(Ordering::Acquire) {
        return Err(err(K::CleanupFailed, "database connection retired"));
    }
    db.execute_batch("BEGIN DEFERRED").map_err(read_err)?;
    let result = operation(&db);
    finish_transaction(&db, &state.retired, result)
}

fn bounds(query: RecordRange) -> Result<(Option<i64>, Option<i64>, i64), StorageError> {
    if query.limit == 0
        || matches!((query.after_sequence, query.before_sequence), (Some(a),Some(b)) if a >= b)
    {
        return Err(err(K::InvalidRange, "invalid record range"));
    }
    let number = |value: Option<u64>| {
        value
            .map(|v| {
                i64::try_from(v)
                    .map_err(|_| err(K::InvalidRange, "record boundary outside SQLite range"))
            })
            .transpose()
    };
    Ok((
        number(query.before_sequence)?,
        number(query.after_sequence)?,
        i64::try_from(query.limit)
            .map_err(|_| err(K::InvalidRange, "record limit outside SQLite range"))?,
    ))
}

fn page_sql(table: &str, order: &str, upper_op: &str) -> String {
    format!("SELECT sequence FROM {table} WHERE sequence>?1 AND sequence{upper_op}?2 ORDER BY sequence {order} LIMIT ?3")
}

fn page<T>(
    db: &Connection,
    table: &str,
    query: RecordRange,
    decode: impl Fn(&Connection, i64) -> Result<T, StorageError>,
) -> Result<RecordPage<T>, StorageError> {
    let (before, after, limit) = bounds(query)?;
    let order = if after.is_some() { "ASC" } else { "DESC" };
    let lower = after.unwrap_or(0);
    let upper = before.unwrap_or(i64::MAX);
    let upper_op = if before.is_some() { "<" } else { "<=" };
    let mut ids = db
        .prepare(&page_sql(table, order, upper_op))
        .map_err(read_err)?
        .query_map(params![lower, upper, limit], |r| r.get::<_, i64>(0))
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?;
    ids.sort_unstable();
    let exists = |id: i64, earlier: bool| {
        let (low, high, include_max) = if earlier {
            (lower, id, false)
        } else {
            (id, upper, before.is_none())
        };
        let op = if include_max { "<=" } else { "<" };
        db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE sequence>?1 AND sequence{op}?2)"),
            params![low, high],
            |r| r.get::<_, bool>(0),
        )
        .map_err(read_err)
    };
    let next_before_sequence = match ids.first() {
        Some(id) if exists(*id, true)? => Some(*id as u64),
        _ => None,
    };
    let next_after_sequence = match ids.last() {
        Some(id) if exists(*id, false)? => Some(*id as u64),
        _ => None,
    };
    let records = ids
        .into_iter()
        .map(|id| decode(db, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RecordPage {
        records,
        next_before_sequence,
        next_after_sequence,
    })
}

pub fn read_message_page(
    storage: &AgentStorage,
    query: RecordRange,
) -> Result<RecordPage<StoredHistoryRecord>, StorageError> {
    read(storage, |db| {
        page(db, "agent_messages", query, |db, id| {
            let (row, turn, time) = db
                .query_row(
                    &format!(
                        "SELECT {},turn,created_at_ms FROM agent_messages WHERE sequence=?1",
                        messages::COLUMNS
                    ),
                    [id],
                    |r| {
                        Ok((
                            messages::from_row(r, 0)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, i64>(7)?,
                        ))
                    },
                )
                .map_err(read_err)?;
            let content = row
                .content
                .clone()
                .ok_or_else(|| schema_err("missing message content"))?;
            let payload = messages::join(row, true)?;
            Ok(StoredHistoryRecord {
                sequence: u64::try_from(id).map_err(schema_err)?,
                record: HistoryRecord {
                    source: None,
                    turn,
                    kind: "message".into(),
                    content,
                    payload,
                    created_at_ms: Some(u64::try_from(time).map_err(schema_err)?),
                },
            })
        })
    })
}

pub fn read_mcl_page(
    storage: &AgentStorage,
    query: RecordRange,
) -> Result<RecordPage<StoredHistoryRecord>, StorageError> {
    read(storage, |db| {
        page(db, "mcl_records", query, |db, id| {
            let (source,turn,content,payload,time) = db.query_row("SELECT source,turn,content,payload,created_at_ms FROM mcl_records WHERE sequence=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?))).map_err(read_err)?;
            let payload = serde_json::from_str(&payload).map_err(schema_err)?;
            validate_payload("mcl", &payload).map_err(schema_err)?;
            Ok(StoredHistoryRecord {
                sequence: u64::try_from(id).map_err(schema_err)?,
                record: HistoryRecord {
                    source: Some(source),
                    turn,
                    kind: "mcl".into(),
                    content,
                    payload,
                    created_at_ms: Some(u64::try_from(time).map_err(schema_err)?),
                },
            })
        })
    })
}

pub fn read_agent_log_page(
    storage: &AgentStorage,
    query: RecordRange,
) -> Result<RecordPage<StoredAgentLog>, StorageError> {
    read(storage, |db| {
        page(db, "agent_logs", query, |db, id| {
            db.query_row("SELECT sequence,created_at_ms,level,source,stage,message,details_json,context_json,origin,raw_error,traceback FROM agent_logs WHERE sequence=?1",[id],|r| {
            Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,Option<String>>(8)?,r.get::<_,Option<String>>(9)?,r.get::<_,Option<String>>(10)?))
        }).map_err(read_err).and_then(|(sequence,time,level,source,stage,message,details,context,origin,raw_error,traceback)| {
            let decode = |v:Option<String>| v.map(|s|serde_json::from_str(&s).map_err(schema_err)).transpose();
            let record = AgentLogRecord {created_at_ms:Some(u64::try_from(time).map_err(schema_err)?),level:match level.as_str(){"debug"=>AgentLogLevel::Debug,"info"=>AgentLogLevel::Info,"warn"=>AgentLogLevel::Warn,"error"=>AgentLogLevel::Error,_=>return Err(schema_err("invalid log level"))},source:match source.as_str(){"driver"=>AgentLogSource::Driver,"tool"=>AgentLogSource::Tool,"host"=>AgentLogSource::Host,_=>return Err(schema_err("invalid log source"))},stage,message,details_json:decode(details)?,context_json:decode(context)?,origin,raw_error,traceback};
            for value in [&record.details_json,&record.context_json] {if value.as_ref().is_some_and(|v|!v.is_object()){return Err(schema_err("invalid log JSON object"));}}
            Ok(StoredAgentLog{sequence:u64::try_from(sequence).map_err(schema_err)?,record})
        })
        })
    })
}

pub(crate) fn record_watermarks(db: &Connection) -> Result<RecordWatermarks, StorageError> {
    let max = |table: &str| {
        db.query_row(
            &format!("SELECT coalesce(max(sequence),0) FROM {table}"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(read_err)
        .and_then(|n| u64::try_from(n).map_err(schema_err))
    };
    Ok(RecordWatermarks {
        message: max("agent_messages")?,
        mcl: max("mcl_records")?,
        agent_log: max("agent_logs")?,
    })
}
pub fn read_record_watermarks(storage: &AgentStorage) -> Result<RecordWatermarks, StorageError> {
    read(storage, record_watermarks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_range_queries_use_primary_key_search_for_all_directions() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agent_messages(sequence INTEGER PRIMARY KEY)")
            .unwrap();
        for order in ["ASC", "DESC"] {
            for upper in ["<", "<="] {
                let sql = page_sql("agent_messages", order, upper);
                let plan = db
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(params![100, 200, 50], |row| row.get::<_, String>(3))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert!(plan.iter().any(|line|line.contains("SEARCH") && line.contains("INTEGER PRIMARY KEY")),"{plan:?}");
                assert!(plan.iter().all(|line| !line.contains("SCAN")), "{plan:?}");
            }
        }
    }
}
