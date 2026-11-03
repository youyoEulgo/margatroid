pub mod components;
pub mod error;
pub mod events;
pub mod types;

pub use components::AgentStorage;
pub use error::{StorageError, StorageErrorKind};
pub use events::{StorageReply, StorageRequest};
pub use types::*;

use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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

const HISTORY: &str = "CREATE TABLE agent_messages(sequence INTEGER PRIMARY KEY,turn TEXT,type TEXT NOT NULL CHECK(length(type)>0),content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));CREATE TABLE mcl_records(sequence INTEGER PRIMARY KEY,source TEXT NOT NULL CHECK(length(source)>0),turn TEXT,content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0));";
const BLOCKS: &str = "CREATE TABLE blocks(name TEXT PRIMARY KEY NOT NULL CHECK(length(name)>0),fields_json TEXT NOT NULL CHECK(json_valid(fields_json) AND json_type(fields_json)='object'));";
const TEMP_BLOCKS: &str = "CREATE TEMP TABLE blocks(name TEXT NOT NULL CHECK(length(name)>0),instance TEXT NOT NULL CHECK(length(instance)>0),fields_json TEXT NOT NULL CHECK(json_valid(fields_json) AND json_type(fields_json)='object'),PRIMARY KEY(name,instance));";

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
            connection: Mutex::new(connection),
            retired: AtomicBool::new(false),
        });
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
    db.execute_batch("BEGIN IMMEDIATE").map_err(write_err)?;
    let outcome = (|| {
        let actual = schema(db)?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(schema_err)?;
        if actual.is_empty() && version == 0 {
            db.execute_batch(&format!("{HISTORY}{BLOCKS}PRAGMA user_version=2;"))
                .map_err(write_err)?;
        } else if version != 2 || actual != expected(&format!("{HISTORY}{BLOCKS}"))? {
            return Err(schema_err(
                "database does not match the current three-table schema",
            ));
        }
        Ok(())
    })();
    finish_transaction(db, &AtomicBool::new(false), outcome)?;
    db.execute_batch("PRAGMA temp_store=MEMORY;")
        .map_err(write_err)?;
    db.execute_batch(TEMP_BLOCKS).map_err(write_err)?;
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
fn namespace(instance: &Option<String>) -> Result<&str, StorageError> {
    match instance {
        Some(s) if s.is_empty() => Err(err(
            K::InvalidRequest,
            "temporary instance must not be empty",
        )),
        Some(s) => Ok(s),
        None => Ok(""),
    }
}
type BlockFields = BTreeMap<String, Vec<serde_json::Value>>;
type ResolvedBlock = (bool, String, BlockFields);
type BlockMessages = (ResolvedBlock, Vec<serde_json::Value>);
fn block(
    db: &Connection,
    name: &str,
    instance: &Option<String>,
) -> Result<ResolvedBlock, StorageError> {
    let ns = namespace(instance)?;
    for (temporary, table, key) in [(true, "temp.blocks", ns), (false, "main.blocks", "")] {
        if temporary && ns.is_empty() {
            continue;
        }
        let json: Option<String> = db
            .query_row(
                &format!(
                    "SELECT fields_json FROM {table} WHERE name=?1{}",
                    if temporary {
                        " AND instance=?2"
                    } else {
                        " AND ?2=''"
                    }
                ),
                params![name, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(read_err)?;
        if let Some(json) = json {
            return Ok((
                temporary,
                key.to_owned(),
                serde_json::from_str(&json).map_err(schema_err)?,
            ));
        }
    }
    Err(err(K::BlockMissing, name))
}
fn messages(db: &Connection, s: &MessageSelector) -> Result<BlockMessages, StorageError> {
    let block = block(db, &s.block, &s.instance)?;
    let values = block
        .2
        .get(&s.field)
        .cloned()
        .ok_or_else(|| err(K::FieldMissing, &s.field))?;
    Ok((block, values))
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
fn selected(db: &Connection, s: &MessageSelector) -> Result<Vec<serde_json::Value>, StorageError> {
    let (_, values) = messages(db, s)?;
    let (a, b) = range(s.selection, values.len(), false)?;
    Ok(values[a..b].to_vec())
}
fn database_operation(
    db: &Connection,
    operation: StorageOperation,
) -> Result<StorageValue, StorageError> {
    match operation {
        StorageOperation::CreateBlock {
            name,
            fields,
            instance,
        } => {
            let ns = namespace(&instance)?;
            let unique: BTreeSet<_> = fields.iter().cloned().collect();
            if name.is_empty()
                || unique.len() != fields.len()
                || fields.iter().any(String::is_empty)
            {
                return Err(err(
                    K::InvalidRequest,
                    "empty or duplicate block/field name",
                ));
            }
            let fields: BlockFields = unique.into_iter().map(|f| (f, Vec::new())).collect();
            let table = if instance.is_some() {
                "temp.blocks"
            } else {
                "main.blocks"
            };
            let other = if instance.is_some() {
                "main.blocks"
            } else {
                "temp.blocks"
            };
            let conflict: bool = db
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {other} WHERE name=?1)"),
                    [&name],
                    |r| r.get(0),
                )
                .map_err(read_err)?;
            if conflict {
                return Err(err(K::LifecycleMismatch, &name));
            }
            let existing: Option<String> = db
                .query_row(
                    &format!(
                        "SELECT fields_json FROM {table} WHERE name=?1{}",
                        if instance.is_some() {
                            " AND instance=?2"
                        } else {
                            " AND ?2=''"
                        }
                    ),
                    params![name, ns],
                    |r| r.get(0),
                )
                .optional()
                .map_err(read_err)?;
            if let Some(json) = existing {
                let stored: BlockFields = serde_json::from_str(&json).map_err(schema_err)?;
                if stored.keys().ne(fields.keys()) {
                    return Err(schema_err(&name));
                }
            } else {
                let sql = if instance.is_some() {
                    "INSERT INTO temp.blocks VALUES(?1,?2,?3)"
                } else {
                    "INSERT INTO main.blocks(name,fields_json) SELECT ?1,?3 WHERE ?2=''"
                };
                db.execute(
                    sql,
                    params![name, ns, serde_json::to_string(&fields).map_err(write_err)?],
                )
                .map_err(write_err)?;
            }
            Ok(StorageValue::Unit)
        }
        StorageOperation::ReadMessages { selectors } => {
            let mut values = Vec::new();
            for s in selectors {
                values.extend(selected(db, &s)?);
            }
            Ok(StorageValue::Messages(values))
        }
        StorageOperation::ReadMessageSources { sources } => {
            let mut values = Vec::new();
            for source in sources {
                values.extend(match source {
                    MessageSource::Stored(selector) => selected(db, &selector)?,
                    MessageSource::Literal(literal) => literal,
                });
            }
            Ok(StorageValue::Messages(values))
        }
        StorageOperation::ReplaceMessages { sources, target } => {
            let ((temporary, ns, mut fields), mut values) = messages(db, &target)?;
            let mut replacement = Vec::new();
            for source in sources {
                replacement.extend(match source {
                    MessageSource::Literal(v) => v,
                    MessageSource::Stored(s) => selected(db, &s)?,
                });
            }
            let (a, b) = range(target.selection, values.len(), true)?;
            values.splice(a..b, replacement);
            fields.insert(target.field, values);
            let table = if temporary {
                "temp.blocks"
            } else {
                "main.blocks"
            };
            db.execute(
                &format!(
                    "UPDATE {table} SET fields_json=?1 WHERE name=?2{}",
                    if temporary {
                        " AND instance=?3"
                    } else {
                        " AND ?3=''"
                    }
                ),
                params![
                    serde_json::to_string(&fields).map_err(write_err)?,
                    target.block,
                    ns
                ],
            )
            .map_err(write_err)?;
            Ok(StorageValue::Unit)
        }
        StorageOperation::DropTemporaryBlocks { instance } => {
            namespace(&Some(instance.clone()))?;
            db.execute("DELETE FROM temp.blocks WHERE instance=?1", [instance])
                .map_err(write_err)?;
            Ok(StorageValue::Unit)
        }
        StorageOperation::AppendHistory { records } => {
            for record in records {
                let table = history_table(&record.kind)?;
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
                let sequence: i64 = db.query_row("SELECT max(sequence) FROM (SELECT coalesce(max(sequence),0) AS sequence FROM agent_messages UNION ALL SELECT coalesce(max(sequence),0) FROM mcl_records)",[],|r| r.get(0)).map_err(read_err)?;
                let sequence = sequence
                    .checked_add(1)
                    .ok_or_else(|| err(K::InvalidRecord, "history sequence exhausted"))?;
                let (columns, source_or_type) = if record.kind == "message" {
                    (
                        "sequence,turn,type,content,payload,created_at_ms",
                        record.payload["type"].as_str(),
                    )
                } else {
                    (
                        "sequence,turn,source,content,payload,created_at_ms",
                        record.source.as_deref(),
                    )
                };
                db.execute(
                    &format!("INSERT INTO {table}({columns}) VALUES(?1,?2,?3,?4,?5,?6)"),
                    params![
                        sequence,
                        record.turn,
                        source_or_type,
                        record.content,
                        serde_json::to_string(&record.payload).map_err(write_err)?,
                        timestamp
                    ],
                )
                .map_err(write_err)?;
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
    let select = |kind: &str, table: &str| {
        let (source, message_type) = if kind == "message" {
            ("NULL", "type")
        } else {
            ("source", "NULL")
        };
        format!(
            "SELECT sequence,{source},turn,'{kind}' AS kind,content,payload,created_at_ms,{message_type} FROM {table}"
        )
    };
    let sql = match kind {
        Some(kind) => select(kind, history_table(kind)?),
        None => format!(
            "{} UNION ALL {}",
            select("message", "agent_messages"),
            select("mcl", "mcl_records")
        ),
    };
    let rows = db
        .prepare(&format!("{sql} ORDER BY sequence"))
        .map_err(read_err)?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?;
    rows.into_iter()
        .map(
            |(sequence, source, turn, kind, content, json, time, message_type)| {
                let payload: serde_json::Value = serde_json::from_str(&json).map_err(schema_err)?;
                validate_payload(&kind, &payload).map_err(schema_err)?;
                if kind == "message" && message_type.as_deref() != payload["type"].as_str() {
                    return Err(schema_err("stored message type does not match payload"));
                }
                Ok(StoredHistoryRecord {
                    sequence: u64::try_from(sequence).map_err(schema_err)?,
                    record: HistoryRecord {
                        source,
                        turn,
                        content,
                        kind,
                        payload,
                        created_at_ms: Some(u64::try_from(time).map_err(schema_err)?),
                    },
                })
            },
        )
        .collect()
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
