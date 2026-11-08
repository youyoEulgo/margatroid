use crate::*;

fn transaction<T>(
    storage: &AgentStorage,
    write: bool,
    operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let connection = storage
        .database
        .shared
        .connection
        .lock()
        .map_err(|_| err(K::Closed, "database lock poisoned"))?;
    if storage.database.shared.retired.load(Ordering::Acquire) {
        return Err(err(K::CleanupFailed, "database connection retired"));
    }
    connection
        .execute_batch(if write {
            "BEGIN IMMEDIATE"
        } else {
            "BEGIN DEFERRED"
        })
        .map_err(if write { write_err } else { read_err })?;
    let outcome = operation(&connection);
    finish_transaction(&connection, &storage.database.shared.retired, outcome)
}

fn validate(record: &AgentLogRecord) -> Result<(), StorageError> {
    if record
        .created_at_ms
        .is_some_and(|time| time > i64::MAX as u64)
    {
        return Err(err(
            K::InvalidRecord,
            "agent log timestamp outside SQLite range",
        ));
    }
    for (name, value) in [
        ("details_json", &record.details_json),
        ("context_json", &record.context_json),
    ] {
        if value.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(err(
                K::InvalidRecord,
                format!("agent log {name} must be a JSON object"),
            ));
        }
    }
    Ok(())
}

/// Append an independent diagnostic record synchronously. Async callers must await
/// the join of a blocking worker that owns a clone of `AgentStorage`.
pub fn append_agent_log(
    storage: &AgentStorage,
    mut record: AgentLogRecord,
) -> Result<StoredAgentLog, StorageError> {
    validate(&record)?;
    transaction(storage, true, |db| {
        let time = match record.created_at_ms {
            Some(time) => time,
            None => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(write_err)?
                .as_millis()
                .try_into()
                .map_err(write_err)?,
        };
        let time = i64::try_from(time).map_err(|e| err(K::InvalidRecord, e))?;
        let maximum: i64 = db
            .query_row(
                "SELECT coalesce(max(sequence),0) FROM agent_logs",
                [],
                |row| row.get(0),
            )
            .map_err(schema_err)?;
        if maximum < 0 {
            return Err(schema_err("invalid stored agent log sequence"));
        }
        let sequence = maximum
            .checked_add(1)
            .ok_or_else(|| err(K::InvalidRecord, "agent log sequence exhausted"))?;
        let level = match record.level {
            AgentLogLevel::Debug => "debug",
            AgentLogLevel::Info => "info",
            AgentLogLevel::Warn => "warn",
            AgentLogLevel::Error => "error",
        };
        let source = match record.source {
            AgentLogSource::Driver => "driver",
            AgentLogSource::Tool => "tool",
            AgentLogSource::Host => "host",
        };
        let encode = |value: &Option<serde_json::Value>| {
            value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(write_err)
        };
        db.execute("INSERT INTO agent_logs(sequence,created_at_ms,level,source,stage,message,details_json,context_json,origin,raw_error,traceback) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![sequence,time,level,source,record.stage,record.message,encode(&record.details_json)?,encode(&record.context_json)?,record.origin,record.raw_error,record.traceback]).map_err(write_err)?;
        record.created_at_ms = Some(time as u64);
        Ok(StoredAgentLog {
            sequence: sequence as u64,
            record,
        })
    })
}

/// Read an ordered snapshot of diagnostic logs, strictly auditing persisted values.
/// Async callers must run this in a blocking worker and await its real join.
pub fn read_agent_logs(
    storage: &AgentStorage,
    after_sequence: Option<u64>,
    limit: usize,
) -> Result<Vec<StoredAgentLog>, StorageError> {
    transaction(storage, false, |db| {
        if limit == 0 || after_sequence.is_some_and(|sequence| sequence >= i64::MAX as u64) {
            return Ok(Vec::new());
        }
        let after = after_sequence.map(|sequence| sequence as i64);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = db.prepare("SELECT sequence,created_at_ms,level,source,stage,message,details_json,context_json,origin,raw_error,traceback FROM agent_logs WHERE (?1 IS NULL OR sequence>?1) ORDER BY sequence LIMIT ?2").map_err(read_err)?;
        let mut rows = statement.query(params![after, limit]).map_err(read_err)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(read_err)? {
            let sequence: i64 = row.get(0).map_err(schema_err)?;
            let time: i64 = row.get(1).map_err(schema_err)?;
            if sequence <= 0 || time < 0 {
                return Err(schema_err("invalid agent log sequence or timestamp"));
            }
            let level = match row.get::<_, String>(2).map_err(schema_err)?.as_str() {
                "debug" => AgentLogLevel::Debug,
                "info" => AgentLogLevel::Info,
                "warn" => AgentLogLevel::Warn,
                "error" => AgentLogLevel::Error,
                _ => return Err(schema_err("invalid stored agent log level")),
            };
            let source = match row.get::<_, String>(3).map_err(schema_err)?.as_str() {
                "driver" => AgentLogSource::Driver,
                "tool" => AgentLogSource::Tool,
                "host" => AgentLogSource::Host,
                _ => return Err(schema_err("invalid stored agent log source")),
            };
            let decode = |index| -> Result<Option<serde_json::Value>, StorageError> {
                let text: Option<String> = row.get(index).map_err(schema_err)?;
                text.map(|text| serde_json::from_str(&text).map_err(schema_err))
                    .transpose()
            };
            let record = AgentLogRecord {
                created_at_ms: Some(time as u64),
                level,
                source,
                stage: row.get(4).map_err(schema_err)?,
                message: row.get(5).map_err(schema_err)?,
                details_json: decode(6)?,
                context_json: decode(7)?,
                origin: row.get(8).map_err(schema_err)?,
                raw_error: row.get(9).map_err(schema_err)?,
                traceback: row.get(10).map_err(schema_err)?,
            };
            validate(&record).map_err(schema_err)?;
            result.push(StoredAgentLog {
                sequence: sequence as u64,
                record,
            });
        }
        Ok(result)
    })
}
