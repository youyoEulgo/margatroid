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

const HISTORY: &str = "CREATE TABLE history_records (sequence INTEGER PRIMARY KEY AUTOINCREMENT,key TEXT NOT NULL UNIQUE CHECK(length(key) > 0),source TEXT NOT NULL,turn TEXT,kind TEXT NOT NULL,content TEXT NOT NULL,payload TEXT NOT NULL,created_at_ms INTEGER NOT NULL DEFAULT 0 CHECK(created_at_ms >= 0));";
const LEGACY_BLOCKS: &str = "CREATE TABLE block_definitions (id INTEGER PRIMARY KEY,agent TEXT NOT NULL,instance TEXT NOT NULL,name TEXT NOT NULL CHECK(length(name) > 0),temporary INTEGER NOT NULL CHECK(temporary IN (0, 1)),schema_json TEXT NOT NULL,UNIQUE(agent, instance, name),CHECK(temporary = 1 OR instance = '')); CREATE TABLE block_fields (definition_id INTEGER NOT NULL REFERENCES block_definitions(id),name TEXT NOT NULL CHECK(length(name) > 0),inner_type TEXT NOT NULL,value_json TEXT NOT NULL,PRIMARY KEY(definition_id, name));";
const BLOCKS: &str = "CREATE TABLE storage_blocks (id INTEGER PRIMARY KEY,name TEXT NOT NULL CHECK(length(name)>0),instance TEXT NOT NULL,temporary INTEGER NOT NULL CHECK(temporary IN (0,1)),fields_json TEXT NOT NULL,UNIQUE(name,instance),CHECK(temporary=1 OR instance='')); CREATE TABLE storage_messages (block_id INTEGER NOT NULL REFERENCES storage_blocks(id),name TEXT NOT NULL CHECK(length(name)>0),value_json TEXT NOT NULL,PRIMARY KEY(block_id,name)); CREATE TABLE storage_metadata (version INTEGER PRIMARY KEY CHECK(version=1));";

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
        let old_history = HISTORY.replace(
            ",created_at_ms INTEGER NOT NULL DEFAULT 0 CHECK(created_at_ms >= 0)",
            "",
        );
        let allowed = expected(&format!("{HISTORY}{LEGACY_BLOCKS}{BLOCKS}"))?;
        let old_expected = expected(&old_history)?;
        for (name, sql) in &actual {
            if allowed.get(name) != Some(sql)
                && !(name == "history_records" && old_expected.get(name) == Some(sql))
            {
                return Err(schema_err(format!(
                    "unsupported schema object {name}; database untouched"
                )));
            }
        }
        let legacy = actual.contains_key("block_definitions");
        if legacy != actual.contains_key("block_fields") {
            return Err(schema_err("incomplete legacy block schema"));
        }
        let current = actual.contains_key("storage_metadata");
        if current {
            for name in [
                "storage_blocks",
                "storage_messages",
                "storage_metadata",
                "history_records",
            ] {
                if actual.get(name) != allowed.get(name) {
                    return Err(schema_err("incomplete storage schema"));
                }
            }
            let versions: Vec<i64> = db
                .prepare("SELECT version FROM storage_metadata")
                .map_err(schema_err)?
                .query_map([], |r| r.get(0))
                .map_err(schema_err)?
                .collect::<rusqlite::Result<_>>()
                .map_err(schema_err)?;
            if versions != [1] {
                return Err(schema_err("unsupported storage version"));
            }
        } else if actual.keys().any(|n| n.starts_with("storage_")) {
            return Err(schema_err("unversioned storage schema"));
        }
        let legacy_messages = if legacy && !current {
            legacy_rows(db)?
        } else {
            Vec::new()
        };
        let integrity: String = db
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(schema_err)?;
        if integrity != "ok" {
            return Err(schema_err(integrity));
        }
        if db
            .prepare("PRAGMA foreign_key_check")
            .map_err(schema_err)?
            .exists([])
            .map_err(schema_err)?
        {
            return Err(schema_err("foreign key violation"));
        }
        if !actual.contains_key("history_records") {
            db.execute_batch(HISTORY).map_err(write_err)?;
        } else if actual.get("history_records") == old_expected.get("history_records") {
            db.execute_batch("ALTER TABLE history_records ADD COLUMN created_at_ms INTEGER NOT NULL DEFAULT 0 CHECK(created_at_ms >= 0)").map_err(write_err)?;
        }
        if !current {
            db.execute_batch(BLOCKS).map_err(write_err)?;
            db.execute("INSERT INTO storage_metadata VALUES(1)", [])
                .map_err(write_err)?;
            for (name, fields) in legacy_messages {
                let names: Vec<_> = fields.keys().cloned().collect();
                db.execute("INSERT INTO storage_blocks(name,instance,temporary,fields_json) VALUES(?1,'',0,?2)", params![name, serde_json::to_string(&names).map_err(write_err)?]).map_err(write_err)?;
                let id = db.last_insert_rowid();
                for (field, value) in fields {
                    db.execute(
                        "INSERT INTO storage_messages VALUES(?1,?2,?3)",
                        params![id, field, serde_json::to_string(&value).map_err(write_err)?],
                    )
                    .map_err(write_err)?;
                }
            }
        }
        Ok(())
    })();
    finish_transaction(db, &AtomicBool::new(false), outcome)
}
type LegacyMessages = Vec<(String, BTreeMap<String, Vec<serde_json::Value>>)>;
fn legacy_rows(db: &Connection) -> Result<LegacyMessages, StorageError> {
    let agents: Vec<String> = db
        .prepare("SELECT DISTINCT agent FROM block_definitions")
        .map_err(schema_err)?
        .query_map([], |r| r.get(0))
        .map_err(schema_err)?
        .collect::<rusqlite::Result<_>>()
        .map_err(schema_err)?;
    if agents.len() > 1 || agents.iter().any(|a| a.is_empty()) {
        return Err(schema_err(
            "legacy database has ambiguous ResourceID namespaces; explicit migration required",
        ));
    }
    let mut statement = db
        .prepare("SELECT id,name,instance,temporary,schema_json FROM block_definitions ORDER BY id")
        .map_err(schema_err)?;
    let definitions = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(schema_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(schema_err)?;
    let mut output = Vec::new();
    for (id, name, instance, temporary, definition) in definitions {
        if name.is_empty()
            || !matches!(temporary, 0 | 1)
            || (temporary == 0 && !instance.is_empty())
        {
            return Err(schema_err("invalid legacy lifecycle"));
        }
        let expected: BTreeMap<String, String> =
            serde_json::from_str(&definition).map_err(schema_err)?;
        let fields = db
            .prepare("SELECT name,inner_type,value_json FROM block_fields WHERE definition_id=?1")
            .map_err(schema_err)?
            .query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(schema_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(schema_err)?;
        if fields.len() != expected.len() {
            return Err(schema_err("legacy fields do not match definition"));
        }
        let mut messages = BTreeMap::new();
        for (field, kind, json) in fields {
            let variant = match kind.as_str() {
                "MESSAGE" => "Message",
                "TOOL" => "Tool",
                "SANDBOX" => "Sandbox",
                _ => return Err(schema_err("unknown legacy field type")),
            };
            if field.is_empty() || expected.get(&field).map(String::as_str) != Some(variant) {
                return Err(schema_err("legacy field type disagrees with schema"));
            }
            let value: serde_json::Value = serde_json::from_str(&json).map_err(schema_err)?;
            let object = value
                .as_object()
                .ok_or_else(|| schema_err("invalid typed legacy value"))?;
            let values = object
                .get(variant)
                .and_then(serde_json::Value::as_array)
                .filter(|_| object.len() == 1)
                .ok_or_else(|| schema_err("invalid typed legacy array"))?;
            if kind == "MESSAGE" {
                messages.insert(field, values.clone());
            }
        }
        if temporary == 0 {
            output.push((name, messages));
        }
    }
    Ok(output)
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
fn block(db: &Connection, name: &str, instance: &Option<String>) -> Result<i64, StorageError> {
    let matches = db
        .prepare("SELECT id FROM storage_blocks WHERE name=?1 AND (instance=?2 OR instance='')")
        .map_err(read_err)?
        .query_map(params![name, namespace(instance)?], |r| r.get::<_, i64>(0))
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?;
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(err(K::BlockMissing, name)),
        _ => Err(err(
            K::LifecycleMismatch,
            "persistent and temporary blocks share a name",
        )),
    }
}
fn messages(
    db: &Connection,
    s: &MessageSelector,
) -> Result<(i64, Vec<serde_json::Value>), StorageError> {
    let id = block(db, &s.block, &s.instance)?;
    let json: String = db
        .query_row(
            "SELECT value_json FROM storage_messages WHERE block_id=?1 AND name=?2",
            params![id, s.field],
            |r| r.get(0),
        )
        .optional()
        .map_err(read_err)?
        .ok_or_else(|| err(K::FieldMissing, &s.field))?;
    Ok((id, serde_json::from_str(&json).map_err(schema_err)?))
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
            let fields: Vec<_> = unique.into_iter().collect();
            let conflict: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM storage_blocks WHERE name=?1 AND temporary<>?2)",
                    params![name, instance.is_some()],
                    |r| r.get(0),
                )
                .map_err(read_err)?;
            if conflict {
                return Err(err(K::LifecycleMismatch, &name));
            }
            let json = serde_json::to_string(&fields).map_err(write_err)?;
            let existing: Option<(i64, String)> = db
                .query_row(
                    "SELECT id,fields_json FROM storage_blocks WHERE name=?1 AND instance=?2",
                    params![name, ns],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(read_err)?;
            if let Some((id, stored)) = existing {
                if stored != json {
                    return Err(schema_err(&name));
                }
                let count: i64 = db
                    .query_row(
                        "SELECT count(*) FROM storage_messages WHERE block_id=?1",
                        [id],
                        |r| r.get(0),
                    )
                    .map_err(read_err)?;
                if count != fields.len() as i64 {
                    return Err(schema_err("block field count mismatch"));
                }
                for field in fields {
                    messages(
                        db,
                        &MessageSelector {
                            block: name.clone(),
                            field,
                            instance: instance.clone(),
                            selection: MessageSelection::All,
                        },
                    )?;
                }
            } else {
                db.execute("INSERT INTO storage_blocks(name,instance,temporary,fields_json) VALUES(?1,?2,?3,?4)",params![name,ns,instance.is_some(),json]).map_err(write_err)?;
                let id = db.last_insert_rowid();
                for field in fields {
                    db.execute(
                        "INSERT INTO storage_messages VALUES(?1,?2,'[]')",
                        params![id, field],
                    )
                    .map_err(write_err)?;
                }
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
            let (id, mut values) = messages(db, &target)?;
            let mut replacement = Vec::new();
            for source in sources {
                replacement.extend(match source {
                    MessageSource::Literal(v) => v,
                    MessageSource::Stored(s) => selected(db, &s)?,
                });
            }
            let (a, b) = range(target.selection, values.len(), true)?;
            values.splice(a..b, replacement);
            db.execute(
                "UPDATE storage_messages SET value_json=?1 WHERE block_id=?2 AND name=?3",
                params![
                    serde_json::to_string(&values).map_err(write_err)?,
                    id,
                    target.field
                ],
            )
            .map_err(write_err)?;
            Ok(StorageValue::Unit)
        }
        StorageOperation::AppendHistory { records } => {
            for record in records {
                if record.key.is_empty()
                    || record.created_at_ms.is_some_and(|t| t > i64::MAX as u64)
                {
                    return Err(err(
                        K::InvalidRecord,
                        "empty key or timestamp outside SQLite range",
                    ));
                }
                let existing=db.query_row("SELECT key,source,turn,kind,content,payload,created_at_ms FROM history_records WHERE key=?1",[&record.key],history_row).optional().map_err(read_err)?.map(decode_history).transpose()?;
                if let Some(mut existing) = existing {
                    existing.created_at_ms = record.created_at_ms;
                    if existing != record {
                        return Err(err(K::KeyConflict, &record.key));
                    }
                    continue;
                }
                let timestamp = match record.created_at_ms {
                    Some(t) => t,
                    None => u64::try_from(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(write_err)?
                            .as_millis(),
                    )
                    .map_err(write_err)?,
                };
                let timestamp = i64::try_from(timestamp).map_err(|e| err(K::InvalidRecord, e))?;
                db.execute("INSERT INTO history_records(key,source,turn,kind,content,payload,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![record.key,record.source,record.turn,record.kind,record.content,serde_json::to_string(&record.payload).map_err(write_err)?,timestamp]).map_err(write_err)?;
            }
            Ok(StorageValue::Unit)
        }
        StorageOperation::ReadHistoryEntries => {
            let rows = db.prepare("SELECT key,source,turn,kind,content,payload,created_at_ms,sequence FROM history_records ORDER BY sequence")
                .map_err(read_err)?
                .query_map([], |row| Ok((row.get::<_, i64>(7)?, history_row(row)?)))
                .map_err(read_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(read_err)?;
            Ok(StorageValue::HistoryEntries(
                rows.into_iter()
                    .map(|(sequence, record)| {
                        Ok(StoredHistoryRecord {
                            sequence: u64::try_from(sequence).map_err(schema_err)?,
                            record: decode_history(record)?,
                        })
                    })
                    .collect::<Result<_, StorageError>>()?,
            ))
        }
        StorageOperation::ReadHistory => {
            let records=db.prepare("SELECT key,source,turn,kind,content,payload,created_at_ms FROM history_records ORDER BY sequence").map_err(read_err)?.query_map([],history_row).map_err(read_err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(read_err)?;
            Ok(StorageValue::History(
                records
                    .into_iter()
                    .map(decode_history)
                    .collect::<Result<_, _>>()?,
            ))
        }
        _ => Err(err(K::InvalidRequest, "not a database operation")),
    }
}
type HistoryRow = (String, String, Option<String>, String, String, String, i64);
fn history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}
fn decode_history(row: HistoryRow) -> Result<HistoryRecord, StorageError> {
    let (key, source, turn, kind, content, payload, time) = row;
    Ok(HistoryRecord {
        key,
        source,
        turn,
        kind,
        content,
        payload: serde_json::from_str(&payload).map_err(schema_err)?,
        created_at_ms: Some(u64::try_from(time).map_err(schema_err)?),
    })
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
