//! Application diagnostics independent of any Agent lifetime.
use crate::{AgentLogRecord, RecordPage, RecordRange, StorageError, StorageErrorKind as K};
use core_plugin::Resource;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA:&str="CREATE TABLE runtime_logs(sequence INTEGER PRIMARY KEY CHECK(sequence>0),report_id TEXT UNIQUE,received_at_ms INTEGER NOT NULL CHECK(received_at_ms>=0),reporter TEXT NOT NULL CHECK(length(reporter)>0),target TEXT NOT NULL CHECK(length(target)>0),agent TEXT,payload TEXT NOT NULL CHECK(json_valid(payload) AND json_type(payload)='object'));";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeLogInput {
    pub report_id: Option<String>,
    pub reporter: String,
    pub target: String,
    pub agent: Option<String>,
    pub record: AgentLogRecord,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredRuntimeLog {
    pub sequence: u64,
    pub received_at_ms: u64,
    #[serde(flatten)]
    pub input: RuntimeLogInput,
}
/// The commit producer invokes this callback after releasing the database lock.
#[derive(Clone)]
pub struct RuntimeLogProbe(Arc<dyn Fn(StoredRuntimeLog) + Send + Sync>);
impl RuntimeLogProbe {
    pub fn new(publish: impl Fn(StoredRuntimeLog) + Send + Sync + 'static) -> Self {
        Self(Arc::new(publish))
    }
}
struct Shared {
    connection: Mutex<Connection>,
    writes: Mutex<()>,
    probe: RuntimeLogProbe,
}
#[derive(Clone)]
pub struct RuntimeLogStore(Arc<Shared>);
impl Resource for RuntimeLogStore {}
fn now() -> Result<u64, StorageError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(crate::write_err)?
        .as_millis()
        .try_into()
        .map_err(crate::write_err)
}

pub fn open_runtime_logs(
    path: &Path,
    probe: RuntimeLogProbe,
) -> Result<RuntimeLogStore, StorageError> {
    let path = crate::absolute(path)?;
    let mut db = Connection::open(path).map_err(crate::write_err)?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(crate::write_err)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(crate::write_err)?;
    let schema = crate::schema(&tx)?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(crate::schema_err)?;
    if schema.is_empty() && version == 0 {
        tx.execute_batch(SCHEMA).map_err(crate::schema_err)?;
        tx.pragma_update(None, "user_version", 1)
            .map_err(crate::schema_err)?;
    } else if version != 1 || schema != crate::expected(SCHEMA)? {
        return Err(crate::schema_err("runtime log database schema mismatch"));
    }
    tx.commit().map_err(crate::write_err)?;
    Ok(RuntimeLogStore(Arc::new(Shared {
        connection: Mutex::new(db),
        writes: Mutex::new(()),
        probe,
    })))
}
fn decode(db: &Connection, sequence: i64) -> Result<StoredRuntimeLog, StorageError> {
    let (received,report_id,reporter,target,agent,payload):(i64,Option<String>,String,String,Option<String>,String)=db.query_row("SELECT received_at_ms,report_id,reporter,target,agent,payload FROM runtime_logs WHERE sequence=?1",[sequence],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(crate::read_err)?;
    let record: AgentLogRecord = serde_json::from_str(&payload).map_err(crate::schema_err)?;
    crate::logs::validate(&record)?;
    if record.created_at_ms.is_none() {
        return Err(crate::schema_err(
            "persisted runtime report is missing its creation time",
        ));
    }
    Ok(StoredRuntimeLog {
        sequence: sequence.try_into().map_err(crate::schema_err)?,
        received_at_ms: received.try_into().map_err(crate::schema_err)?,
        input: RuntimeLogInput {
            report_id,
            reporter,
            target,
            agent,
            record,
        },
    })
}
impl RuntimeLogStore {
    pub fn append(&self, mut input: RuntimeLogInput) -> Result<StoredRuntimeLog, StorageError> {
        crate::logs::validate(&input.record)?;
        if input.reporter.is_empty()
            || input.target.is_empty()
            || input.report_id.as_ref().is_some_and(|id| id.is_empty())
        {
            return Err(crate::err(K::InvalidRecord, "missing runtime log identity"));
        }
        let _write = self
            .0
            .writes
            .lock()
            .map_err(|_| crate::err(K::Closed, "runtime log commit gate poisoned"))?;
        let mut db = self
            .0
            .connection
            .lock()
            .map_err(|_| crate::err(K::Closed, "runtime log database lock poisoned"))?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(crate::write_err)?;
        if let Some(id) = &input.report_id {
            if let Some(sequence) = tx
                .query_row(
                    "SELECT sequence FROM runtime_logs WHERE report_id=?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(crate::read_err)?
            {
                let existing = decode(&tx, sequence)?;
                tx.commit().map_err(crate::write_err)?;
                return Ok(existing);
            }
        }
        let received = now()?;
        if input.record.created_at_ms.is_none() {
            input.record.created_at_ms = Some(received);
        }
        let payload = serde_json::to_string(&input.record).map_err(crate::write_err)?;
        tx.execute("INSERT INTO runtime_logs(report_id,received_at_ms,reporter,target,agent,payload) VALUES(?1,?2,?3,?4,?5,?6)",params![input.report_id,i64::try_from(received).map_err(crate::write_err)?,input.reporter,input.target,input.agent,payload]).map_err(crate::write_err)?;
        let sequence = tx.last_insert_rowid();
        tx.commit().map_err(crate::write_err)?;
        let stored = StoredRuntimeLog {
            sequence: sequence.try_into().map_err(crate::write_err)?,
            received_at_ms: received,
            input,
        };
        drop(db);
        (self.0.probe.0)(stored.clone());
        Ok(stored)
    }
    pub fn read(&self, range: RecordRange) -> Result<RecordPage<StoredRuntimeLog>, StorageError> {
        let mut db = self
            .0
            .connection
            .lock()
            .map_err(|_| crate::err(K::Closed, "runtime log database lock poisoned"))?;
        let tx = db.transaction().map_err(crate::read_err)?;
        let page = crate::pages::page(&tx, "runtime_logs", range, decode)?;
        tx.commit().map_err(crate::read_err)?;
        Ok(page)
    }
}
