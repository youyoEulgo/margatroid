mod pages;
pub use pages::{read_agent_log_page, read_mcl_page, read_message_page, read_record_watermarks};
mod blocks;
mod logs;
mod messages;

pub use blocks::{read_block_snapshots, restore_block, snapshot_block};
pub use logs::{append_agent_log, read_agent_logs};
pub use messages::append_message_history;
pub mod components;
pub mod error;
pub mod events;
pub mod types;

pub use components::AgentStorage;
pub use error::{StorageError, StorageErrorKind};
pub use events::{StorageReply, StorageRequest};
pub use types::*;

use rusqlite::{params, Connection};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use StorageErrorKind as K;

pub const SCHEMA_VERSION: i64 = 5;
const AGENT_LOGS: &str = "CREATE TABLE agent_logs(sequence INTEGER PRIMARY KEY CHECK(sequence>0),created_at_ms INTEGER NOT NULL CHECK(typeof(created_at_ms)='integer' AND created_at_ms>=0),level TEXT NOT NULL CHECK(typeof(level)='text' AND level IN ('debug','info','warn','error')),source TEXT NOT NULL CHECK(typeof(source)='text' AND source IN ('driver','tool','host')),stage TEXT NOT NULL CHECK(typeof(stage)='text'),message TEXT NOT NULL CHECK(typeof(message)='text'),details_json TEXT CHECK(details_json IS NULL OR (typeof(details_json)='text' AND json_valid(details_json) AND json_type(details_json)='object')),context_json TEXT CHECK(context_json IS NULL OR (typeof(context_json)='text' AND json_valid(context_json) AND json_type(context_json)='object')),origin TEXT CHECK(origin IS NULL OR typeof(origin)='text'),raw_error TEXT CHECK(raw_error IS NULL OR typeof(raw_error)='text'),traceback TEXT CHECK(traceback IS NULL OR typeof(traceback)='text'));";
const HISTORY: &str = "CREATE TABLE agent_messages(sequence INTEGER PRIMARY KEY CHECK(sequence>0),turn TEXT,role TEXT NOT NULL CHECK(length(role)>0),content TEXT NOT NULL,data TEXT,tool_call TEXT,payload TEXT NOT NULL CHECK(json_valid(payload) AND json_type(payload)='object'),created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));CREATE TABLE mcl_records(sequence INTEGER PRIMARY KEY,source TEXT NOT NULL CHECK(length(source)>0),turn TEXT,content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));";

fn err(kind: K, message: impl std::fmt::Display) -> StorageError {
    StorageError::new(kind, message)
}
fn read_err(e: impl std::fmt::Display) -> StorageError {
    err(K::ReadFailed, e)
}
fn write_err(e: impl std::fmt::Display) -> StorageError {
    err(K::WriteFailed, e)
}
fn schema_err(e: impl std::fmt::Display) -> StorageError {
    err(K::SchemaMismatch, e)
}
fn normalize(sql: &str) -> String {
    let mut normalized = String::new();
    let mut quote = None;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(end) = quote {
            normalized.push(c);
            if c == end {
                if chars.peek() == Some(&end) {
                    normalized.push(chars.next().unwrap());
                } else {
                    quote = None;
                }
            }
        } else if matches!(c, '\'' | '"' | '`' | '[') {
            quote = Some(if c == '[' { ']' } else { c });
            normalized.push(c);
        } else if !c.is_whitespace() && c != ';' {
            normalized.extend(c.to_lowercase());
        }
    }
    normalized
}
fn schema(db: &Connection) -> Result<BTreeMap<String, String>, StorageError> {
    db.prepare("SELECT name,coalesce(sql,'') FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' ORDER BY name").map_err(schema_err)?
        .query_map([], |r| Ok((r.get(0)?, normalize(&r.get::<_, String>(1)?)))).map_err(schema_err)?
        .collect::<rusqlite::Result<_>>().map_err(schema_err)
}
fn expected(sql: &str) -> Result<BTreeMap<String, String>, StorageError> {
    let db = Connection::open_in_memory().map_err(schema_err)?;
    db.execute_batch(sql).map_err(schema_err)?;
    schema(&db)
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn absolute(path: &Path) -> Result<PathBuf, StorageError> {
    fs::create_dir_all(parent(path)).map_err(write_err)?;
    if path.exists() {
        fs::canonicalize(path).map_err(read_err)
    } else {
        Ok(fs::canonicalize(parent(path)).map_err(read_err)?.join(
            path.file_name()
                .ok_or_else(|| err(K::InvalidRequest, "path has no filename"))?,
        ))
    }
}

pub fn open_storage(
    database_path: &Path,
    settings_path: &Path,
    default_settings: &str,
) -> Result<AgentStorage, StorageError> {
    static DATABASES: OnceLock<Mutex<HashMap<PathBuf, Weak<types::DatabaseState>>>> =
        OnceLock::new();
    static SETTINGS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let db_path = absolute(database_path)?;
    let settings_path = absolute(settings_path)?;
    if db_path == settings_path {
        return Err(err(
            K::InvalidRequest,
            "database and settings paths must differ",
        ));
    }
    let mut registry = DATABASES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| err(K::Closed, "database registry poisoned"))?;
    let shared = if let Some(shared) = registry.get(&db_path).and_then(Weak::upgrade) {
        shared
    } else {
        let mut connection = Connection::open(&db_path).map_err(write_err)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(write_err)?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(write_err)?;
        initialize(&mut connection)?;
        let shared = Arc::new(types::DatabaseState {
            block_root: {
                let mut path = db_path.as_os_str().to_os_string();
                path.push(".blocks");
                PathBuf::from(path)
            },
            connection: Mutex::new(connection),
            retired: AtomicBool::new(false),
            temporary: Mutex::new(BTreeMap::new()),
        });
        {
            let db = shared
                .connection
                .lock()
                .map_err(|_| err(K::Closed, "database lock poisoned"))?;
            blocks::initialize(&db, &shared.block_root, &shared.retired)?;
        }
        registry.insert(db_path, Arc::downgrade(&shared));
        shared
    };
    let database = StorageDatabase { shared };
    drop(registry);
    let mut locks = SETTINGS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| err(K::Closed, "settings registry poisoned"))?;
    let settings_lock = locks
        .get(&settings_path)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let lock = Arc::new(Mutex::new(()));
            locks.insert(settings_path.clone(), Arc::downgrade(&lock));
            lock
        });
    drop(locks);
    {
        let _guard = settings_lock
            .lock()
            .map_err(|_| err(K::Closed, "settings lock poisoned"))?;
        if !settings_path.exists() {
            parse_config(default_settings)?;
            atomic_write(&settings_path, default_settings.as_bytes(), false)?;
        }
        read_config(&settings_path)?;
    }
    Ok(AgentStorage {
        database,
        settings_path,
        settings_lock,
    })
}

fn initialize(db: &mut Connection) -> Result<(), StorageError> {
    let mapping = blocks::SCHEMA;
    db.execute_batch("BEGIN IMMEDIATE").map_err(write_err)?;
    let outcome = (|| {
        let actual = schema(db)?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(schema_err)?;
        if actual.is_empty() && version == 0 {
            db.execute_batch(&format!(
                "{HISTORY}{mapping}{AGENT_LOGS}PRAGMA user_version={SCHEMA_VERSION};"
            ))
            .map_err(write_err)?;
        } else if version != SCHEMA_VERSION
            || actual != expected(&format!("{HISTORY}{mapping}{AGENT_LOGS}"))?
        {
            return Err(schema_err(
                "database does not match the current four-table schema (version 5)",
            ));
        }
        Ok(())
    })();
    finish_transaction(db, &AtomicBool::new(false), outcome)?;

    Ok(())
}

fn finish_transaction<T>(
    db: &Connection,
    retired: &AtomicBool,
    outcome: Result<T, StorageError>,
) -> Result<T, StorageError> {
    let outcome = outcome.and_then(|value| {
        db.execute_batch("COMMIT").map_err(|e| match &e {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::SystemIoFailure
                        | rusqlite::ErrorCode::DiskFull
                        | rusqlite::ErrorCode::CannotOpen
                ) =>
            {
                err(K::CommitUnknown, e)
            }
            _ => write_err(e),
        })?;
        Ok(value)
    });
    if outcome.is_err() && !db.is_autocommit() {
        if let Err(e) = db.execute_batch("ROLLBACK") {
            retired.store(true, Ordering::Release);
            return Err(err(
                K::CleanupFailed,
                format!("{}; rollback failed: {e}", outcome.err().unwrap()),
            ));
        }
    }
    if !db.is_autocommit() {
        retired.store(true, Ordering::Release);
        return Err(err(K::CleanupFailed, "transaction remains open"));
    }
    outcome
}

pub fn execute_storage(
    storage: &AgentStorage,
    operation: StorageOperation,
) -> Result<StorageValue, StorageError> {
    if matches!(
        operation,
        StorageOperation::ReadConfig
            | StorageOperation::SetConfig { .. }
            | StorageOperation::DeleteConfig { .. }
    ) {
        let _guard = storage
            .settings_lock
            .lock()
            .map_err(|_| err(K::Closed, "settings lock poisoned"))?;
        return config_operation(&storage.settings_path, operation);
    }
    let connection = storage
        .database
        .shared
        .connection
        .lock()
        .map_err(|_| err(K::Closed, "database lock poisoned"))?;
    if storage.database.shared.retired.load(Ordering::Acquire) {
        return Err(err(K::CleanupFailed, "database connection retired"));
    }
    if blocks::is_operation(&operation) {
        return blocks::execute(
            &connection,
            &storage.database.shared.block_root,
            &storage.database.shared.retired,
            &mut *storage
                .database
                .shared
                .temporary
                .lock()
                .map_err(|_| err(K::Closed, "TEMP lock poisoned"))?,
            operation,
        );
    }
    let read_only = matches!(
        operation,
        StorageOperation::ReadMessages { .. }
            | StorageOperation::ReadMessageSources { .. }
            | StorageOperation::ReadHistory
            | StorageOperation::ReadHistoryEntries
            | StorageOperation::ReadAgentMessages
            | StorageOperation::ReadMclRecords
    );
    connection
        .execute_batch(if read_only {
            "BEGIN DEFERRED"
        } else {
            "BEGIN IMMEDIATE"
        })
        .map_err(write_err)?;
    let outcome = database_operation(&connection, operation);
    finish_transaction(&connection, &storage.database.shared.retired, outcome)
}
fn range(
    selection: MessageSelection,
    len: usize,
    write: bool,
) -> Result<(usize, usize), StorageError> {
    let invalid = || err(K::InvalidRange, "message selection outside array");
    let len = i64::try_from(len).map_err(|_| invalid())?;
    let endpoint = |n: i64| {
        if n < 0 {
            len.checked_add(1).and_then(|l| l.checked_add(n))
        } else {
            Some(n)
        }
    };
    let (start, end) = match selection {
        MessageSelection::All => (0, len),
        MessageSelection::Index(0) if len == 0 && !write => (0, 0),
        MessageSelection::Index(n) => {
            let end = endpoint(n).ok_or_else(invalid)?;
            if write {
                (end, end)
            } else if n < 0 {
                (end.checked_sub(1).ok_or_else(invalid)?, end)
            } else {
                (end, end.checked_add(1).ok_or_else(invalid)?)
            }
        }
        MessageSelection::Range(a, b) => {
            if (a < 0) != (b < 0) {
                return Err(invalid());
            }
            (
                endpoint(a).ok_or_else(invalid)?,
                endpoint(b).ok_or_else(invalid)?,
            )
        }
    };
    if start < 0 || start > end || end > len {
        return Err(invalid());
    }
    Ok((start as usize, end as usize))
}
fn database_operation(
    db: &Connection,
    operation: StorageOperation,
) -> Result<StorageValue, StorageError> {
    match operation {
        StorageOperation::AppendHistory { records } => {
            for record in records {
                history_table(&record.kind)?;
                validate_payload(&record.kind, &record.payload)?;
                if record.kind == "mcl"
                    && record.source.as_deref().is_none_or(|source| {
                        source.is_empty() || source.chars().any(char::is_control)
                    })
                {
                    return Err(err(
                        K::InvalidRecord,
                        "MCL source must be nonempty and contain no control characters",
                    ));
                }
                if record.created_at_ms.is_some_and(|t| t > i64::MAX as u64) {
                    return Err(err(K::InvalidRecord, "timestamp outside SQLite range"));
                }
                let timestamp = record.created_at_ms.unwrap_or(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(write_err)?
                        .as_millis()
                        .try_into()
                        .map_err(write_err)?,
                );
                let timestamp = i64::try_from(timestamp).map_err(|e| err(K::InvalidRecord, e))?;
                if record.kind == "message" {
                    messages::append(db, &record.payload, record.turn.as_deref(), timestamp)?;
                } else {
                    let sequence = messages::next_sequence(db)?;
                    db.execute("INSERT INTO mcl_records(sequence,turn,source,content,payload,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![sequence,record.turn,record.source,record.content,serde_json::to_string(&record.payload).map_err(write_err)?,timestamp]).map_err(write_err)?;
                }
            }
            Ok(StorageValue::Unit)
        }
        StorageOperation::ReadHistoryEntries => {
            Ok(StorageValue::HistoryEntries(read_entries(db, None)?))
        }
        StorageOperation::ReadAgentMessages => Ok(StorageValue::HistoryEntries(read_entries(
            db,
            Some("message"),
        )?)),
        StorageOperation::ReadMclRecords => {
            Ok(StorageValue::HistoryEntries(read_entries(db, Some("mcl"))?))
        }
        StorageOperation::ReadHistory => Ok(StorageValue::History(
            read_entries(db, None)?
                .into_iter()
                .map(|e| e.record)
                .collect(),
        )),
        _ => Err(err(K::InvalidRequest, "not a database operation")),
    }
}
fn history_table(kind: &str) -> Result<&'static str, StorageError> {
    match kind {
        "message" => Ok("agent_messages"),
        "mcl" => Ok("mcl_records"),
        _ => Err(err(K::InvalidRecord, "unsupported history kind")),
    }
}
fn validate_payload(kind: &str, payload: &serde_json::Value) -> Result<(), StorageError> {
    let valid = payload.is_object()
        && match kind {
            "message" => payload
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            "mcl" => payload
                .get("text")
                .is_some_and(serde_json::Value::is_string),
            _ => false,
        };
    if valid {
        Ok(())
    } else {
        Err(err(K::InvalidRecord, "invalid history payload shape"))
    }
}
fn read_entries(
    db: &Connection,
    kind: Option<&str>,
) -> Result<Vec<StoredHistoryRecord>, StorageError> {
    let mut result = Vec::new();
    if kind.is_none() || kind == Some("message") {
        let mut query = db
            .prepare(&format!(
                "SELECT {},turn,created_at_ms FROM agent_messages ORDER BY sequence",
                messages::COLUMNS
            ))
            .map_err(read_err)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    messages::from_row(r, 0)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })
            .map_err(read_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(read_err)?;
        for (row, turn, timestamp) in rows {
            let sequence =
                row.sequence
                    .ok_or_else(|| schema_err("missing history sequence"))? as u64;
            let content = row.content.clone().unwrap_or_default();
            result.push(StoredHistoryRecord {
                sequence,
                record: HistoryRecord {
                    source: None,
                    turn,
                    kind: "message".into(),
                    content,
                    payload: messages::join(row, true)?,
                    created_at_ms: Some(timestamp as u64),
                },
            });
        }
    }
    if kind.is_none() || kind == Some("mcl") {
        let mut query=db.prepare("SELECT sequence,source,turn,content,payload,created_at_ms FROM mcl_records ORDER BY sequence").map_err(read_err)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(read_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(read_err)?;
        for (sequence, source, turn, content, payload, timestamp) in rows {
            let payload = serde_json::from_str(&payload).map_err(schema_err)?;
            validate_payload("mcl", &payload).map_err(schema_err)?;
            result.push(StoredHistoryRecord {
                sequence: sequence as u64,
                record: HistoryRecord {
                    source: Some(source),
                    turn,
                    kind: "mcl".into(),
                    content,
                    payload,
                    created_at_ms: Some(timestamp as u64),
                },
            });
        }
    }
    result.sort_by_key(|r| r.sequence);
    Ok(result)
}

fn parse_config(text: &str) -> Result<toml::Table, StorageError> {
    toml::from_str(text).map_err(|e| err(K::ConfigInvalid, e))
}
fn read_config(path: &Path) -> Result<toml::Table, StorageError> {
    parse_config(&fs::read_to_string(path).map_err(read_err)?)
}
fn config_operation(
    path: &Path,
    operation: StorageOperation,
) -> Result<StorageValue, StorageError> {
    let mut table = read_config(path)?;
    let (key, value) = match operation {
        StorageOperation::ReadConfig => return Ok(StorageValue::Config(table)),
        StorageOperation::SetConfig { path, value } => (path, Some(value)),
        StorageOperation::DeleteConfig { path } => (path, None),
        _ => return Err(err(K::InvalidRequest, "not a settings operation")),
    };
    let parts: Vec<_> = key.split('.').collect();
    if parts
        .iter()
        .any(|s| s.is_empty() || s.trim() != *s || s.contains(['[', ']']))
    {
        return Err(err(K::ConfigPathInvalid, key));
    }
    let mut current = &mut table;
    for part in &parts[..parts.len() - 1] {
        if !current.contains_key(*part) {
            if value.is_none() {
                return Ok(StorageValue::Unit);
            }
            current.insert((*part).into(), toml::Value::Table(toml::Table::new()));
        }
        current = current
            .get_mut(*part)
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| err(K::ConfigTypeMismatch, *part))?;
    }
    let last = parts[parts.len() - 1];
    if let Some(value) = value {
        current.insert(last.into(), value);
    } else {
        current.remove(last);
    }
    let serialized = toml::to_string(&table).map_err(|e| err(K::ConfigInvalid, e))?;
    parse_config(&serialized)?;
    atomic_write(path, serialized.as_bytes(), true)?;
    Ok(StorageValue::Unit)
}
fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), StorageError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut attempts = 0;
    let (temp, mut file) = loop {
        let temp = parent(path).join(format!(
            ".storage-{}-{}-{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(write_err)?
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 10 => {
                attempts += 1;
            }
            Err(e) => return Err(write_err(e)),
        }
    };
    let outcome = (|| {
        file.write_all(bytes).map_err(write_err)?;
        file.sync_all().map_err(write_err)?;
        drop(file);
        if replace {
            fs::rename(&temp, path).map_err(write_err)?;
        } else {
            match fs::hard_link(&temp, path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(write_err(e)),
            }
            fs::remove_file(&temp).map_err(|e| err(K::CleanupFailed, e))?;
        }
        File::open(parent(path))
            .and_then(|f| f.sync_all())
            .map_err(|e| err(K::CommitUnknown, e))?;
        Ok(())
    })();
    if temp.exists() {
        fs::remove_file(&temp)
            .map_err(|e| err(K::CleanupFailed, format!("temporary cleanup failed: {e}")))?;
    }
    outcome
}
