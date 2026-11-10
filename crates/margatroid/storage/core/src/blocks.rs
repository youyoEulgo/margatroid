mod publication;

use super::*;
use rusqlite::{backup::Backup, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) const SCHEMA: &str = "CREATE TABLE block_snapshots(record_id INTEGER PRIMARY KEY,block_name TEXT NOT NULL CHECK(length(block_name)>0),instance TEXT NOT NULL CHECK(instance=''),kind TEXT NOT NULL CHECK(kind IN ('timeline','snapshot')),snapshot_id INTEGER NOT NULL CHECK(snapshot_id>=0),file_name TEXT NOT NULL UNIQUE,fields_json TEXT NOT NULL CHECK(json_valid(fields_json) AND json_type(fields_json)='object'),upstream_id INTEGER CHECK(upstream_id>0),restore_intent TEXT CHECK(restore_intent IS NULL OR (json_valid(restore_intent) AND json_type(restore_intent)='object')),UNIQUE(block_name,instance,snapshot_id),CHECK((kind='timeline' AND snapshot_id=0) OR (kind='snapshot' AND snapshot_id>0 AND restore_intent IS NULL)),CHECK(upstream_id IS NULL OR upstream_id<>snapshot_id));";
const BLOCK_VERSION: i64 = 2;
type Fields = BTreeMap<String, String>;
pub(crate) type MemoryBlocks = BTreeMap<(String, String), MemoryBlock>;
#[derive(Clone)]
struct MemorySnapshot {
    fields: BTreeMap<String, Vec<Value>>,
    upstream: Option<u64>,
}
pub(crate) struct MemoryBlock {
    fields: BTreeMap<String, Vec<Value>>,
    upstream: Option<u64>,
    snapshots: BTreeMap<u64, MemorySnapshot>,
}
impl MemoryBlock {
    fn new(fields: Vec<String>) -> Result<Self, StorageError> {
        let names: BTreeSet<_> = fields.iter().cloned().collect();
        if names.len() != fields.len() || names.iter().any(String::is_empty) {
            return Err(err(K::InvalidRequest, "empty or duplicate TEMP child"));
        }
        Ok(Self {
            fields: names.into_iter().map(|n| (n, Vec::new())).collect(),
            upstream: None,
            snapshots: BTreeMap::new(),
        })
    }
    fn snapshot(&mut self) -> Result<(), StorageError> {
        let id = self
            .snapshots
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|id| *id <= i64::MAX as u64)
            .ok_or_else(|| err(K::InvalidRequest, "snapshot identifiers exhausted"))?;
        self.snapshots.insert(
            id,
            MemorySnapshot {
                fields: self.fields.clone(),
                upstream: self.upstream,
            },
        );
        self.upstream = Some(id);
        Ok(())
    }
    fn restore(&mut self, id: u64) -> Result<(), StorageError> {
        let target = self
            .snapshots
            .get(&id)
            .cloned()
            .ok_or_else(|| err(K::InvalidRequest, "snapshot does not belong to TEMP Block"))?;
        self.snapshot()?;
        self.fields = target.fields;
        self.upstream = Some(id);
        Ok(())
    }
    fn query(&self) -> Value {
        let mapping = |fields: &BTreeMap<String, Vec<Value>>| {
            fields
                .keys()
                .enumerate()
                .map(|(i, n)| (n.clone(), Value::String(format!("f{i}"))))
                .collect::<serde_json::Map<String, Value>>()
        };
        json!({"timeline":{"file_name":null,"fields_json":mapping(&self.fields),"upstream_id":self.upstream},"snapshots":self.snapshots.iter().map(|(id,s)|json!({"snapshot_id":id,"file_name":null,"fields_json":mapping(&s.fields),"upstream_id":s.upstream})).collect::<Vec<_>>()})
    }
}
fn memory_block_mut<'a>(
    memory: &'a mut MemoryBlocks,
    name: &str,
    instance: Option<&str>,
) -> Result<Option<&'a mut MemoryBlock>, StorageError> {
    valid_name(name)?;
    namespace(instance)?;
    Ok(instance.and_then(|i| memory.get_mut(&(i.to_owned(), name.to_owned()))))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    name: String,
    instance: String,
    id: i64,
    file: String,
    fields: Fields,
    upstream: Option<i64>,
    intent: Option<Intent>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    archive_id: i64,
    target_id: i64,
    target_fields: Fields,
}
fn hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn directory(name: &str, _instance: &str) -> String {
    name.to_owned()
}
fn filename(name: &str, instance: &str, id: i64) -> String {
    format!(
        "{}/{}",
        directory(name, instance),
        if id == 0 {
            "timeline.sqlite".to_owned()
        } else {
            format!("snapshot_{id}.sqlite")
        }
    )
}
fn block_path(root: &Path, mapping: &Mapping) -> Result<PathBuf, StorageError> {
    valid_name(&mapping.name).map_err(schema_err)?;
    if !mapping.instance.is_empty()
        || mapping.file != filename(&mapping.name, &mapping.instance, mapping.id)
    {
        return Err(schema_err("invalid Block file mapping"));
    }
    let path = root.join(&mapping.file);
    // Reject filesystem aliases: all managed components must be actual directories/files.
    let mut current = root.to_path_buf();
    for component in Path::new(&mapping.file).components() {
        current.push(component);
        if let Ok(meta) = fs::symlink_metadata(&current) {
            if meta.file_type().is_symlink() {
                return Err(schema_err("symlink in Block path"));
            }
        }
    }
    if fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(schema_err("symlink Block root"));
    }
    Ok(path)
}
fn mappings(db: &Connection) -> Result<Vec<Mapping>, StorageError> {
    let rows = db.prepare("SELECT block_name,instance,snapshot_id,file_name,fields_json,upstream_id,restore_intent FROM block_snapshots ORDER BY block_name,instance,snapshot_id").map_err(read_err)?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<String>>(6)?))).map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(read_err)?;
    rows.into_iter()
        .map(|(name, instance, id, file, fields, upstream, intent)| {
            Ok(Mapping {
                name,
                instance,
                id,
                file,
                fields: serde_json::from_str(&fields).map_err(schema_err)?,
                upstream,
                intent: intent
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(schema_err)?,
            })
        })
        .collect()
}
fn valid_name(name: &str) -> Result<(), StorageError> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(err(K::InvalidRequest, "invalid Block identifier"));
    }
    Ok(())
}
fn namespace(instance: Option<&str>) -> Result<&str, StorageError> {
    match instance {
        Some("") => Err(err(K::InvalidRequest, "empty TEMP instance")),
        Some(value) => Ok(value),
        None => Ok(""),
    }
}
fn resolve(db: &Connection, name: &str, instance: Option<&str>) -> Result<Mapping, StorageError> {
    valid_name(name)?;
    let ns = namespace(instance)?;
    let all = mappings(db)?;
    let mapping = all
        .iter()
        .find(|m| m.name == name && m.id == 0 && m.instance == ns)
        .or_else(|| {
            all.iter()
                .find(|m| m.name == name && m.id == 0 && m.instance.is_empty())
        })
        .cloned()
        .ok_or_else(|| err(K::BlockMissing, name))?;
    validate_fields(&mapping.fields)?;
    if mapping.intent.is_some() {
        return Err(err(K::CleanupFailed, "Block has unfinished restore"));
    }
    Ok(mapping)
}
fn field_sql(table: &str) -> String {
    format!("CREATE TABLE {table}(position INTEGER PRIMARY KEY CHECK(position>=0),sequence INTEGER CHECK(sequence>0),role TEXT NOT NULL,content TEXT NOT NULL,data TEXT,tool_call TEXT,payload TEXT NOT NULL CHECK(json_valid(payload) AND json_type(payload)='object'));")
}
fn validate_fields(fields: &Fields) -> Result<(), StorageError> {
    for (index, (name, table)) in fields.iter().enumerate() {
        if name.is_empty() || table != &format!("f{index}") {
            return Err(schema_err("invalid Block child mapping"));
        }
    }
    Ok(())
}
fn validate_file(path: &Path, fields: &Fields) -> Result<(), StorageError> {
    validate_fields(fields)?;
    let db =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(schema_err)?;
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(schema_err)?;
    let sql: String = fields.values().map(|table| field_sql(table)).collect();
    if version != BLOCK_VERSION || schema(&db)? != expected(&sql)? {
        return Err(schema_err("Block database schema mismatch"));
    }
    let check: String = db
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(schema_err)?;
    if check != "ok" {
        return Err(schema_err(check));
    }
    for table in fields.values() {
        let valid:bool=db.query_row(&format!("SELECT count(*)=coalesce(max(position)+1,0) AND coalesce(min(position),0)=0 FROM {table}"),[],|r|r.get(0)).map_err(schema_err)?;
        if !valid {
            return Err(schema_err("noncontiguous Block positions"));
        }
    }
    Ok(())
}
fn sync_file(path: &Path) -> Result<(), StorageError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(write_err)?;
    sync_directory(parent(path))
}
fn sync_directory(path: &Path) -> Result<(), StorageError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| err(K::CommitUnknown, e))
}
fn insert(db: &Connection, m: &Mapping) -> Result<(), StorageError> {
    db.execute("INSERT INTO block_snapshots(block_name,instance,kind,snapshot_id,file_name,fields_json,upstream_id) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![m.name,m.instance,if m.id==0 {"timeline"} else {"snapshot"},m.id,m.file,serde_json::to_string(&m.fields).map_err(write_err)?,m.upstream]).map_err(write_err)?;
    Ok(())
}
fn transaction<T>(
    db: &Connection,
    retired: &AtomicBool,
    operation: impl FnOnce() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    db.execute_batch("BEGIN IMMEDIATE").map_err(write_err)?;
    let result = finish_transaction(db, retired, operation());
    if result.as_ref().is_err_and(|e| e.kind == K::CommitUnknown) {
        retired.store(true, Ordering::Release);
    }
    result
}
fn copy_database(source: &Path, destination: &Path, fields: &Fields) -> Result<(), StorageError> {
    validate_file(source, fields)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(write_err)?;
    let source =
        Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(read_err)?;
    let mut target = Connection::open(destination).map_err(write_err)?;
    {
        let backup = Backup::new(&source, &mut target).map_err(write_err)?;
        backup
            .run_to_completion(128, Duration::from_millis(1), None)
            .map_err(write_err)?;
    }
    target.close().map_err(|(_, e)| write_err(e))?;
    validate_file(destination, fields)?;
    sync_file(destination)
}
fn next_id(db: &Connection, m: &Mapping) -> Result<i64, StorageError> {
    let max: i64 = db
        .query_row(
            "SELECT max(snapshot_id) FROM block_snapshots WHERE block_name=?1 AND instance=?2",
            params![m.name, m.instance],
            |r| r.get(0),
        )
        .map_err(read_err)?;
    max.checked_add(1)
        .ok_or_else(|| err(K::InvalidRequest, "Block snapshot identifiers exhausted"))
}
fn snapshot_mapping(m: &Mapping, id: i64) -> Mapping {
    Mapping {
        id,
        file: filename(&m.name, &m.instance, id),
        intent: None,
        ..m.clone()
    }
}
fn locked<T>(
    storage: &AgentStorage,
    operation: impl FnOnce(
        &Connection,
        &Path,
        &AtomicBool,
        &mut MemoryBlocks,
    ) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let state = &storage.database.shared;
    let db = state
        .connection
        .lock()
        .map_err(|_| err(K::Closed, "database lock poisoned"))?;
    if state.retired.load(Ordering::Acquire) {
        return Err(err(K::CleanupFailed, "database connection retired"));
    }
    let mut memory = state
        .temporary
        .lock()
        .map_err(|_| err(K::Closed, "TEMP lock poisoned"))?;
    operation(&db, &state.block_root, &state.retired, &mut memory)
}
pub fn snapshot_block(
    storage: &AgentStorage,
    name: &str,
    instance: Option<&str>,
) -> Result<(), StorageError> {
    locked(storage, |db, root, retired, memory| {
        if let Some(block) = memory_block_mut(memory, name, instance)? {
            return block.snapshot();
        }
        let current = resolve(db, name, instance)?;
        let snapshot = snapshot_mapping(&current, next_id(db, &current)?);
        let source = block_path(root, &current)?;
        let destination = block_path(root, &snapshot)?;
        publication::begin(root, &snapshot)?;
        if let Err(failed) = copy_database(&source, &destination, &current.fields) {
            publication::abandon(root, &snapshot)?;
            return Err(failed);
        }
        let result = transaction(db, retired, || {
            insert(db, &snapshot)?;
            db.execute("UPDATE block_snapshots SET upstream_id=?1 WHERE block_name=?2 AND instance=?3 AND snapshot_id=0",params![snapshot.id,current.name,current.instance]).map_err(write_err)?;
            Ok(())
        });
        if result.is_ok() {
            if let Err(failed) = publication::finish(root, &snapshot) {
                retired.store(true, Ordering::Release);
                return Err(failed);
            }
        } else if !retired.load(Ordering::Acquire) {
            publication::abandon(root, &snapshot)?;
        }
        result
    })
}
pub fn read_block_snapshots(
    storage: &AgentStorage,
    name: &str,
    instance: Option<&str>,
) -> Result<Value, StorageError> {
    locked(storage, |db, root, _, memory| {
        if let Some(block) = memory_block_mut(memory, name, instance)? {
            return Ok(block.query());
        }
        let current = resolve(db, name, instance)?;
        let value = |m: &Mapping| json!({"file_name":m.file,"fields_json":m.fields,"upstream_id":m.upstream});
        let mut snapshots = Vec::new();
        for m in mappings(db)?
            .into_iter()
            .filter(|m| m.name == current.name && m.instance == current.instance && m.id > 0)
        {
            block_path(root, &m)?;
            let mut v = value(&m);
            v["snapshot_id"] = json!(m.id);
            snapshots.push(v);
        }
        Ok(json!({"timeline":value(&current),"snapshots":snapshots}))
    })
}
pub fn restore_block(
    storage: &AgentStorage,
    name: &str,
    instance: Option<&str>,
    snapshot_id: u64,
) -> Result<(), StorageError> {
    locked(storage, |db, root, retired, memory| {
        if let Some(block) = memory_block_mut(memory, name, instance)? {
            return block.restore(snapshot_id);
        }
        let mut current = resolve(db, name, instance)?;
        let id = i64::try_from(snapshot_id).map_err(|e| err(K::InvalidRequest, e))?;
        let target = mappings(db)?
            .into_iter()
            .find(|m| {
                m.name == current.name && m.instance == current.instance && m.id == id && id > 0
            })
            .ok_or_else(|| err(K::InvalidRequest, "snapshot does not belong to Block"))?;
        validate_file(&block_path(root, &target)?, &target.fields)?;
        let work = block_path(root, &current)?;
        validate_file(&work, &current.fields)?;
        let connection = Connection::open_with_flags(&work, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(write_err)?;
        let (busy, _, _): (i64, i64, i64) = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(write_err)?;
        if busy != 0 {
            return Err(write_err("Block checkpoint is busy"));
        }
        connection.close().map_err(|(_, e)| write_err(e))?;
        sync_file(&work)?;
        let intent = Intent {
            archive_id: next_id(db, &current)?,
            target_id: id,
            target_fields: target.fields,
        };
        let archive_path = block_path(root, &snapshot_mapping(&current, intent.archive_id))?;
        if archive_path.exists() {
            return Err(write_err("unregistered restore archive already exists"));
        }
        transaction(db, retired, || {
            db.execute("UPDATE block_snapshots SET restore_intent=?1 WHERE block_name=?2 AND instance=?3 AND snapshot_id=0",params![serde_json::to_string(&intent).map_err(write_err)?,current.name,current.instance]).map_err(write_err)?;
            Ok(())
        })?;
        current.intent = Some(intent);
        let result = finish_restore(db, root, retired, &current);
        if result.is_err() {
            retired.store(true, Ordering::Release);
        }
        result
    })
}
fn finish_restore(
    db: &Connection,
    root: &Path,
    retired: &AtomicBool,
    current: &Mapping,
) -> Result<(), StorageError> {
    let intent = current
        .intent
        .as_ref()
        .ok_or_else(|| schema_err("missing restore intent"))?;
    let all = mappings(db)?;
    let target = all
        .iter()
        .find(|m| {
            m.name == current.name
                && m.instance == current.instance
                && m.id == intent.target_id
                && m.id > 0
        })
        .ok_or_else(|| schema_err("restore target missing"))?;
    if target.fields != intent.target_fields
        || intent.archive_id <= 0
        || all.iter().any(|m| {
            m.name == current.name && m.instance == current.instance && m.id == intent.archive_id
        })
    {
        return Err(schema_err("invalid restore intent"));
    }
    let archive = snapshot_mapping(current, intent.archive_id);
    let archive_path = block_path(root, &archive)?;
    let work = block_path(root, current)?;
    let target_path = block_path(root, target)?;
    validate_file(&target_path, &target.fields)?;
    if !archive_path.exists() {
        validate_file(&work, &current.fields)?;
        fs::rename(&work, &archive_path).map_err(write_err)?;
        sync_directory(parent(&work))?;
    }
    validate_file(&archive_path, &archive.fields)?;
    // This path belongs exclusively to the durable intent and can be regenerated.
    let temp = work.with_extension("restore");
    if fs::symlink_metadata(&temp).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(schema_err("symlink restore temporary"));
    }
    if temp.exists() {
        fs::remove_file(&temp).map_err(write_err)?;
    }
    copy_database(&target_path, &temp, &target.fields)?;
    fs::rename(&temp, &work).map_err(write_err)?;
    sync_file(&work)?;
    transaction(db, retired, || {
        insert(db, &archive)?;
        db.execute("UPDATE block_snapshots SET upstream_id=?1,fields_json=?2,restore_intent=NULL WHERE block_name=?3 AND instance=?4 AND snapshot_id=0",params![target.id,serde_json::to_string(&target.fields).map_err(write_err)?,current.name,current.instance]).map_err(write_err)?;
        Ok(())
    })
}
pub(crate) fn initialize(
    db: &Connection,
    root: &Path,
    retired: &AtomicBool,
) -> Result<(), StorageError> {
    publication::recover(db, root)?;
    for m in mappings(db)?
        .into_iter()
        .filter(|m| m.id == 0 && m.intent.is_some())
    {
        finish_restore(db, root, retired, &m)?;
    }
    let all = mappings(db)?;
    for m in &all {
        valid_name(&m.name).map_err(schema_err)?;
        if m.upstream.is_some_and(|id| {
            !all.iter()
                .any(|p| p.name == m.name && p.instance == m.instance && p.id == id && id > 0)
        }) {
            return Err(schema_err("missing snapshot ancestor"));
        }
        let mut seen = BTreeSet::new();
        let mut cursor = m.upstream;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(schema_err("cyclic snapshot ancestry"));
            }
            cursor = all
                .iter()
                .find(|p| p.name == m.name && p.instance == m.instance && p.id == id)
                .and_then(|p| p.upstream);
        }
        if !all
            .iter()
            .any(|p| p.name == m.name && p.instance == m.instance && p.id == 0)
        {
            return Err(schema_err("snapshot without timeline"));
        }
        validate_file(&block_path(root, m)?, &m.fields)?;
    }
    Ok(())
}
pub(crate) fn is_operation(op: &StorageOperation) -> bool {
    matches!(
        op,
        StorageOperation::CreateBlock { .. }
            | StorageOperation::ReadMessages { .. }
            | StorageOperation::ReadMessageSources { .. }
            | StorageOperation::ReplaceMessages { .. }
            | StorageOperation::DropTemporaryBlocks { .. }
    )
}
fn create(
    db: &Connection,
    root: &Path,
    retired: &AtomicBool,
    name: String,
    fields: Vec<String>,
    instance: Option<String>,
) -> Result<(), StorageError> {
    valid_name(&name)?;
    let ns = namespace(instance.as_deref())?;
    let unique: BTreeSet<_> = fields.iter().collect();
    if unique.len() != fields.len() || fields.iter().any(String::is_empty) {
        return Err(err(K::InvalidRequest, "empty or duplicate Block child"));
    }
    let all = mappings(db)?;
    if all
        .iter()
        .any(|m| m.name == name && m.id == 0 && (m.instance.is_empty() != ns.is_empty()))
    {
        return Err(err(K::LifecycleMismatch, &name));
    }
    let fields: Fields = unique
        .into_iter()
        .enumerate()
        .map(|(n, name)| (name.clone(), format!("f{n}")))
        .collect();
    if let Some(existing) = all
        .iter()
        .find(|m| m.name == name && m.instance == ns && m.id == 0)
    {
        if existing.intent.is_some() {
            return Err(err(K::CleanupFailed, "unfinished restore"));
        }
        if existing.fields != fields {
            return Err(schema_err(&name));
        }
        return validate_file(&block_path(root, existing)?, &fields);
    }
    let m = Mapping {
        name,
        instance: ns.to_owned(),
        id: 0,
        file: String::new(),
        fields,
        upstream: None,
        intent: None,
    };
    let m = Mapping {
        file: filename(&m.name, &m.instance, 0),
        ..m
    };
    let path = block_path(root, &m)?;
    publication::begin(root, &m)?;
    let outcome = (|| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(write_err)?;
        let connection = Connection::open(&path).map_err(write_err)?;
        let sql: String = m.fields.values().map(|table| field_sql(table)).collect();
        connection
            .execute_batch(&format!(
                "BEGIN IMMEDIATE;{sql}PRAGMA user_version={BLOCK_VERSION};COMMIT;"
            ))
            .map_err(write_err)?;
        connection.close().map_err(|(_, e)| write_err(e))?;
        sync_file(&path)?;
        transaction(db, retired, || insert(db, &m))
    })();
    if outcome.is_ok() {
        if let Err(failed) = publication::finish(root, &m) {
            retired.store(true, Ordering::Release);
            return Err(failed);
        }
    } else if !retired.load(Ordering::Acquire) {
        publication::abandon(root, &m)?;
    }
    outcome
}
fn selected(
    db: &Connection,
    root: &Path,
    memory: &MemoryBlocks,
    selector: &MessageSelector,
) -> Result<Vec<Value>, StorageError> {
    if let Some(instance) = selector.instance.as_deref() {
        namespace(Some(instance))?;
        if let Some(block) = memory.get(&(instance.to_owned(), selector.block.clone())) {
            let values = block
                .fields
                .get(&selector.field)
                .ok_or_else(|| err(K::FieldMissing, &selector.field))?;
            let (a, b) = range(selector.selection, values.len(), false)?;
            return Ok(values[a..b].to_vec());
        }
    }
    let m = resolve(db, &selector.block, selector.instance.as_deref())?;
    let table = m
        .fields
        .get(&selector.field)
        .ok_or_else(|| err(K::FieldMissing, &selector.field))?;
    let path = block_path(root, &m)?;
    let connection =
        Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(read_err)?;
    let values = field_values(&connection, table)?;
    let (a, b) = range(selector.selection, values.len(), false)?;
    Ok(values[a..b].to_vec())
}
fn field_values(connection: &Connection, table: &str) -> Result<Vec<Value>, StorageError> {
    let rows = connection
        .prepare(&format!(
            "SELECT {} FROM {table} ORDER BY position",
            messages::COLUMNS
        ))
        .map_err(read_err)?
        .query_map([], |r| messages::from_row(r, 0))
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?;
    rows.into_iter().map(|r| messages::join(r, true)).collect()
}

fn source_values(
    db: &Connection,
    root: &Path,
    memory: &MemoryBlocks,
    sources: Vec<MessageSource>,
) -> Result<Vec<Value>, StorageError> {
    let mut values = Vec::new();
    for source in sources {
        values.extend(match source {
            MessageSource::Stored(s) => selected(db, root, memory, &s)?,
            MessageSource::Literal(v) => v,
        });
    }
    Ok(values)
}
pub(crate) fn execute(
    db: &Connection,
    root: &Path,
    retired: &AtomicBool,
    memory: &mut MemoryBlocks,
    op: StorageOperation,
) -> Result<StorageValue, StorageError> {
    match op {
        StorageOperation::CreateBlock {
            name,
            fields,
            instance,
        } => {
            valid_name(&name)?;
            if let Some(instance) = instance {
                namespace(Some(&instance))?;
                if mappings(db)?.iter().any(|m| m.name == name) {
                    return Err(err(K::LifecycleMismatch, name));
                }
                let key = (instance, name);
                let block = MemoryBlock::new(fields)?;
                if let Some(old) = memory.get(&key) {
                    if old.fields.keys().ne(block.fields.keys()) {
                        return Err(schema_err("TEMP child mismatch"));
                    }
                } else {
                    memory.insert(key, block);
                }
            } else {
                if memory.keys().any(|(_, n)| n == &name) {
                    return Err(err(K::LifecycleMismatch, name));
                }
                create(db, root, retired, name, fields, None)?;
            }
        }
        StorageOperation::ReadMessages { selectors } => {
            return Ok(StorageValue::Messages(source_values(
                db,
                root,
                memory,
                selectors.into_iter().map(MessageSource::Stored).collect(),
            )?))
        }
        StorageOperation::ReadMessageSources { sources } => {
            return Ok(StorageValue::Messages(source_values(
                db, root, memory, sources,
            )?))
        }
        StorageOperation::ReplaceMessages { sources, target } => {
            let replacement = source_values(db, root, memory, sources)?;
            for value in &replacement {
                messages::validate_sequence(db, value)?;
            }
            if let Some(block) =
                memory_block_mut(memory, &target.block, target.instance.as_deref())?
            {
                let values = block
                    .fields
                    .get_mut(&target.field)
                    .ok_or_else(|| err(K::FieldMissing, &target.field))?;
                let (a, b) = range(target.selection, values.len(), true)?;
                values.splice(a..b, replacement);
                return Ok(StorageValue::Unit);
            }
            let m = resolve(db, &target.block, target.instance.as_deref())?;
            let table = m
                .fields
                .get(&target.field)
                .ok_or_else(|| err(K::FieldMissing, &target.field))?;
            let connection = Connection::open_with_flags(
                block_path(root, &m)?,
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .map_err(write_err)?;
            transaction(&connection, retired, || {
                let mut values = field_values(&connection, table)?;
                let (a, b) = range(target.selection, values.len(), true)?;
                values.splice(a..b, replacement);
                connection
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(write_err)?;
                let mut statement = connection
                    .prepare(&format!(
                        "INSERT INTO {table}(position,sequence,role,content,data,tool_call,payload) VALUES(?1,?2,?3,?4,?5,?6,?7)"
                    ))
                    .map_err(write_err)?;
                for (i, value) in values.iter().enumerate() {
                    let row = messages::split(value)?;
                    statement
                        .execute(params![
                            i as i64,
                            row.sequence,
                            row.role,
                            row.content,
                            row.data,
                            row.tool_call,
                            row.payload
                        ])
                        .map_err(write_err)?;
                }
                Ok(())
            })?;
        }
        StorageOperation::DropTemporaryBlocks { instance } => {
            namespace(Some(&instance))?;
            memory.retain(|(owner, _), _| owner != &instance);
        }
        _ => return Err(err(K::InvalidRequest, "not a Block operation")),
    }
    Ok(StorageValue::Unit)
}
