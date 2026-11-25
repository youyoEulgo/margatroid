use crate::{
    AgentCategory, WorkspaceAgent, WorkspaceError, WorkspaceErrorKind as K, WorkspaceMetadata,
    WorkspaceRecordsSnapshot,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{watch, Notify};

const TEMP_PREFIX: &str = "~temporary-";
const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
CREATE TABLE workspace (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 address TEXT NOT NULL, alias TEXT NOT NULL, project_root TEXT NOT NULL
) STRICT;
CREATE TABLE agents (
 id TEXT PRIMARY KEY NOT NULL,
 parent_node TEXT NOT NULL,
 node_name TEXT UNIQUE,
 alias TEXT,
 category TEXT NOT NULL CHECK(category IN ('Normal','Temporary','Archived')),
 directory_name TEXT NOT NULL,
 runtime_name TEXT NOT NULL,
 tool_path TEXT NOT NULL,
 tool_arguments TEXT NOT NULL CHECK(json_valid(tool_arguments) AND json_type(tool_arguments)='object'),
 previous_node_name TEXT,
 previous_alias TEXT,
 CHECK(length(directory_name)>0 AND directory_name NOT IN ('.','..') AND instr(directory_name,'/')=0 AND instr(directory_name,char(92))=0),
 CHECK((category='Normal' AND node_name IS NOT NULL AND directory_name=node_name AND runtime_name=node_name)
    OR (category='Temporary' AND node_name IS NULL AND directory_name=id AND runtime_name='~temporary-'||id)
    OR (category='Archived' AND node_name IS NULL))
) STRICT;
CREATE TABLE archive_intents (
 agent_id TEXT PRIMARY KEY NOT NULL REFERENCES agents(id),
 source_category TEXT NOT NULL CHECK(source_category IN ('Normal','Temporary')),
 source_directory TEXT NOT NULL,
 destination_directory TEXT NOT NULL UNIQUE
) STRICT;
PRAGMA user_version=1;
"#;
const SELECT: &str = "SELECT id,parent_node,node_name,alias,category,directory_name,runtime_name,tool_path,tool_arguments,previous_node_name,previous_alias FROM agents";

#[derive(Default)]
struct Admission {
    accepting: bool,
    nodes: BTreeSet<String>,
}
#[derive(Clone)]
struct Prepared {
    before: Option<WorkspaceAgent>,
    candidate: WorkspaceAgent,
}

/// A single node operation; wrap in Arc to retain across supervised workers.
/// Drop releases admission only after the last Arc has been dropped.
pub struct WorkspaceOperation {
    key: String,
    connection: Arc<Mutex<Option<Connection>>>,
    admission: Arc<Mutex<Admission>>,
    idle: Arc<Notify>,
    prepared: Mutex<Option<Prepared>>,
}
impl std::fmt::Debug for WorkspaceOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceOperation")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
impl WorkspaceOperation {
    pub fn key(&self) -> &str {
        &self.key
    }
}
impl Drop for WorkspaceOperation {
    fn drop(&mut self) {
        let mut admission = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        admission.nodes.remove(&self.key);
        drop(admission);
        self.idle.notify_waiters();
    }
}
#[derive(Clone)]
pub struct WorkspaceDatabase {
    path: PathBuf,
    connection: Arc<Mutex<Option<Connection>>>,
    admission: Arc<Mutex<Admission>>,
    idle: Arc<Notify>,
    changes: Arc<watch::Sender<Option<WorkspaceRecordsSnapshot>>>,
}
impl std::fmt::Debug for WorkspaceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceDatabase")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}
impl WorkspaceDatabase {
    /// Creates only a new database file in an already prepared managed layout.
    /// Never opens an existing database, reads YAML, or migrates a schema.
    pub fn create(
        path: impl AsRef<Path>,
        metadata: WorkspaceMetadata,
    ) -> Result<Self, WorkspaceError> {
        validate_metadata(&metadata)?;
        let path = checked_database_path(path.as_ref(), false)?;
        validate_layout(path.parent().unwrap())?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(db_error)?;
        let mut conn = connect(&path)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute_batch(SCHEMA).map_err(db_error)?;
        tx.execute(
            "INSERT INTO workspace VALUES(1,?1,?2,?3)",
            params![
                metadata.address,
                metadata.alias,
                path_text(&metadata.project_root)?
            ],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        File::open(&path)
            .and_then(|f| f.sync_all())
            .map_err(db_error)?;
        sync_dir(path.parent().unwrap()).map_err(db_error)?;
        Self::from_connection(path, metadata, conn)
    }
    /// Opens an existing v1 database. Pending archive intents are recovered
    /// before a first committed snapshot or handle is exposed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let path = checked_database_path(path.as_ref(), true)?;
        validate_layout(path.parent().unwrap())?;
        let conn = connect(&path)?;
        validate_schema(&conn)?;
        let metadata = conn
            .query_row(
                "SELECT address,alias,project_root FROM workspace WHERE singleton=1",
                [],
                |r| {
                    Ok(WorkspaceMetadata {
                        address: r.get(0)?,
                        alias: r.get(1)?,
                        project_root: PathBuf::from(r.get::<_, String>(2)?),
                    })
                },
            )
            .map_err(db_error)?;
        validate_metadata(&metadata)?;
        let db = Self::from_connection(path, metadata, conn)?;
        db.recover_archives()?;
        // Recovery may have advanced an internal version, but nothing has yet
        // escaped open(). The current watch value is the confirmed state.
        Ok(db)
    }
    fn from_connection(
        path: PathBuf,
        metadata: WorkspaceMetadata,
        conn: Connection,
    ) -> Result<Self, WorkspaceError> {
        let agents = read_agents(&conn)?;
        for agent in &agents {
            validate_record(agent, &metadata.address)?;
        }
        let (changes, _) = watch::channel(Some(WorkspaceRecordsSnapshot {
            revision: 0,
            metadata,
            agents,
        }));
        Ok(Self {
            path,
            connection: Arc::new(Mutex::new(Some(conn))),
            admission: Arc::new(Mutex::new(Admission {
                accepting: true,
                nodes: BTreeSet::new(),
            })),
            idle: Arc::new(Notify::new()),
            changes: Arc::new(changes),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn metadata(&self) -> Result<WorkspaceMetadata, WorkspaceError> {
        self.changes
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.metadata.clone())
            .ok_or_else(|| error(K::Closed, "workspace database is closed"))
    }
    pub fn rename_workspace_alias(&self, alias: String) -> Result<(), WorkspaceError> {
        validate_alias(Some(&alias))?;
        let _operation = self.admit_operation(":workspace-alias")?;
        let mut connection = self.conn()?;
        let tx = connection
            .as_mut()
            .unwrap()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut metadata = read_metadata(&tx)?;
        if metadata.alias == alias {
            return Ok(());
        }
        tx.execute("UPDATE workspace SET alias=?1 WHERE singleton=1", [&alias])
            .map_err(db_error)?;
        metadata.alias = alias;
        let agents = read_agents(&tx)?;
        tx.commit().map_err(db_error)?;
        self.publish_committed(metadata, agents);
        Ok(())
    }
    pub fn rename_agent_alias(
        &self,
        agent_id: &str,
        alias: Option<String>,
    ) -> Result<(), WorkspaceError> {
        validate_alias(alias.as_deref())?;
        if agent_id.trim().is_empty() || agent_id.chars().any(char::is_control) {
            return Err(error(K::InvalidRequest, "invalid Agent record id"));
        }
        if !self.is_accepting() {
            return Err(error(K::Closed, "workspace no longer accepts operations"));
        }
        // Read the committed identity without holding the SQL lock while taking admission.
        // The transaction below rechecks it after admission, covering archive/reuse races.
        let key = self
            .changes
            .borrow()
            .as_ref()
            .ok_or_else(|| error(K::Closed, "workspace database is closed"))?
            .agents
            .iter()
            .find(|record| record.id == agent_id)
            .map(|record| {
                record
                    .node_name
                    .as_deref()
                    .unwrap_or(&record.runtime_name)
                    .to_owned()
            })
            .ok_or_else(|| error(K::AgentRecordMissing, "Agent record missing"))?;
        let _operation = self.try_operation(&key)?;
        let mut connection = self.conn()?;
        let tx = connection
            .as_mut()
            .unwrap()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let record = by_id(&tx, agent_id)?
            .ok_or_else(|| error(K::AgentRecordMissing, "Agent record missing"))?;
        if record.node_name.as_deref().unwrap_or(&record.runtime_name) != key {
            return Err(error(K::Busy, "Agent record changed during admission"));
        }
        if read_intent(&tx, agent_id)?.is_some() {
            return Err(error(K::ArchivePending, "Agent archive is pending"));
        }
        if record.alias == alias {
            return Ok(());
        }
        tx.execute(
            "UPDATE agents SET alias=?1 WHERE id=?2",
            params![alias, agent_id],
        )
        .map_err(db_error)?;
        let metadata = read_metadata(&tx)?;
        let agents = read_agents(&tx)?;
        tx.commit().map_err(db_error)?;
        self.publish_committed(metadata, agents);
        Ok(())
    }
    pub fn managed_directory(&self) -> &Path {
        self.path.parent().unwrap()
    }
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.connection, &other.connection)
    }
    pub fn subscribe_changes(&self) -> watch::Receiver<Option<WorkspaceRecordsSnapshot>> {
        self.changes.subscribe()
    }
    pub fn is_open(&self) -> bool {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }
    pub fn is_accepting(&self) -> bool {
        self.admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .accepting
    }
    pub fn try_operation(&self, key: &str) -> Result<WorkspaceOperation, WorkspaceError> {
        validate_operation_key(key)?;
        self.admit_operation(key)
    }
    fn admit_operation(&self, key: &str) -> Result<WorkspaceOperation, WorkspaceError> {
        let mut admission = lock(&self.admission)?;
        if !admission.accepting {
            return Err(error(K::Closed, "workspace no longer accepts operations"));
        }
        if !admission.nodes.insert(key.to_owned()) {
            return Err(error(K::Busy, "node already has an operation"));
        }
        Ok(WorkspaceOperation {
            key: key.into(),
            connection: self.connection.clone(),
            admission: self.admission.clone(),
            idle: self.idle.clone(),
            prepared: Mutex::new(None),
        })
    }
    /// Generate a temporary operation key using SQLite random bytes; it is not
    /// a persistent registration until commit_agent succeeds.
    pub fn temporary_operation_key(&self) -> Result<String, WorkspaceError> {
        let guard = self.conn()?;
        Ok(format!(
            "{TEMP_PREFIX}{}",
            random_id(guard.as_ref().unwrap())?
        ))
    }
    pub fn close_admission(&self) {
        self.admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .accepting = false;
    }
    pub async fn wait_idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .admission
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .nodes
                .is_empty()
            {
                return;
            }
            notified.await;
        }
    }
    pub fn list_agents(&self) -> Result<Vec<WorkspaceAgent>, WorkspaceError> {
        let c = self.conn()?;
        read_agents(c.as_ref().unwrap())
    }
    pub fn agent_by_id(&self, id: &str) -> Result<WorkspaceAgent, WorkspaceError> {
        let c = self.conn()?;
        by_id(c.as_ref().unwrap(), id)?.ok_or_else(|| error(K::Missing, "Agent record missing"))
    }
    pub fn agent_by_node(&self, node: &str) -> Result<WorkspaceAgent, WorkspaceError> {
        validate_operation_key(node)?;
        let c = self.conn()?;
        find_node(c.as_ref().unwrap(), node)?.ok_or_else(|| error(K::Missing, "Agent node missing"))
    }
    pub fn ensure_startable(&self, id: &str) -> Result<(), WorkspaceError> {
        let c = self.conn()?;
        let c = c.as_ref().unwrap();
        let agent = by_id(c, id)?.ok_or_else(|| error(K::Missing, "Agent record missing"))?;
        check_startable(c, &agent)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_agent(
        &self,
        operation: &WorkspaceOperation,
        parent_node: String,
        node_name: Option<String>,
        alias: Option<String>,
        tool_path: PathBuf,
        arguments: serde_json::Value,
    ) -> Result<WorkspaceAgent, WorkspaceError> {
        self.validate_operation(operation)?;
        validate_parent(&parent_node, &self.metadata()?.address)?;
        validate_absolute(&tool_path)?;
        if !arguments.is_object() {
            return Err(error(
                K::InvalidRequest,
                "Driver arguments must be a JSON object",
            ));
        }
        validate_alias(alias.as_deref())?;
        let expected = match node_name.as_deref() {
            Some(node) => {
                validate_node_name(node)?;
                node
            }
            None => {
                if !operation.key.starts_with(TEMP_PREFIX) {
                    return Err(error(
                        K::InvalidRequest,
                        "temporary Agent requires a temporary operation key",
                    ));
                }
                &operation.key
            }
        };
        if operation.key != expected {
            return Err(error(
                K::InvalidRequest,
                "operation key differs from target node",
            ));
        }
        let mut prepared = lock(&operation.prepared)?;
        if prepared.is_some() {
            return Err(error(K::Busy, "operation already prepared an Agent"));
        }
        let c = self.conn()?;
        let c = c.as_ref().unwrap();
        let before = find_node(c, expected)?;
        if node_name.is_none() && before.is_some() {
            return Err(error(
                K::DuplicateNode,
                "temporary identity already registered",
            ));
        }
        let candidate = if let Some(old) = &before {
            check_startable(c, old)?;
            if old.tool_path != tool_path {
                return Err(error(
                    K::DriverMismatch,
                    "node belongs to a different Driver",
                ));
            }
            if old.parent_node != parent_node {
                return Err(error(
                    K::ParentMismatch,
                    "node belongs to a different parent",
                ));
            }
            let mut updated = old.clone();
            // Existing identity/display values are authoritative in the registry.
            // A Driver invocation may have captured older arguments before a rename.
            updated.tool_arguments = arguments;
            updated
        } else {
            let id = if node_name.is_none() {
                operation.key[TEMP_PREFIX.len()..].to_owned()
            } else {
                random_id(c)?
            };
            if by_id(c, &id)?.is_some() {
                return Err(error(K::DuplicateNode, "record identity collision"));
            }
            let category = if node_name.is_some() {
                AgentCategory::Normal
            } else {
                AgentCategory::Temporary
            };
            let directory_name = node_name.clone().unwrap_or_else(|| id.clone());
            let runtime_name = node_name
                .clone()
                .unwrap_or_else(|| format!("{TEMP_PREFIX}{id}"));
            let candidate = WorkspaceAgent {
                id,
                parent_node,
                tool_path,
                tool_arguments: arguments,
                node_name,
                alias,
                category,
                directory_name,
                runtime_name,
                previous_node_name: None,
                previous_alias: None,
            };
            let dir = self.agent_directory(&candidate)?;
            if fs::symlink_metadata(&dir).is_ok() {
                return Err(error(
                    K::UnregisteredDirectory,
                    "Agent directory exists without a record",
                )
                .with_directory(dir));
            }
            candidate
        };
        validate_record(&candidate, &self.metadata()?.address)?;
        *prepared = Some(Prepared {
            before,
            candidate: candidate.clone(),
        });
        Ok(candidate)
    }
    pub fn commit_agent(
        &self,
        operation: &WorkspaceOperation,
        record: WorkspaceAgent,
    ) -> Result<WorkspaceAgent, WorkspaceError> {
        self.validate_operation(operation)?;
        let prepared = lock(&operation.prepared)?;
        let prepared = prepared
            .as_ref()
            .ok_or_else(|| error(K::InvalidRequest, "operation has no prepared Agent"))?;
        if prepared.candidate != record {
            return Err(error(K::InvalidRequest, "prepared Agent materials changed"));
        }
        validate_record(&record, &self.metadata()?.address)?;
        // Files have been prepared by the caller, but the candidate cannot be
        // committed if its root has become an unsafe link or disappeared.
        let dir = self.agent_directory(&record)?;
        require_directory(&dir)?;
        let mut c = self.conn()?;
        let c = c.as_mut().unwrap();
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let current = by_id(&tx, &record.id)?;
        let node = record.node_name.as_deref().unwrap_or(&record.runtime_name);
        let current_node = find_node(&tx, node)?;
        if current != prepared.before || current_node != prepared.before {
            return Err(error(
                K::DuplicateNode,
                "registration changed since preparation",
            ));
        }
        if let Some(old) = &current {
            check_startable(&tx, old)?;
        }
        tx.execute("INSERT INTO agents(id,parent_node,node_name,alias,category,directory_name,runtime_name,tool_path,tool_arguments,previous_node_name,previous_alias) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET alias=excluded.alias,tool_arguments=excluded.tool_arguments", params![record.id,record.parent_node,record.node_name,record.alias,record.category.as_str(),record.directory_name,record.runtime_name,path_text(&record.tool_path)?,record.tool_arguments.to_string(),record.previous_node_name,record.previous_alias]).map_err(db_error)?;
        let agents = read_agents(&tx)?;
        let metadata = read_metadata(&tx)?;
        tx.commit().map_err(db_error)?;
        self.publish_committed(metadata, agents);
        Ok(record)
    }
    pub fn agent_directory(&self, record: &WorkspaceAgent) -> Result<PathBuf, WorkspaceError> {
        validate_record(record, &self.metadata()?.address)?;
        let base = self
            .managed_directory()
            .join("agents")
            .join(record.category.directory());
        reject_symlinks(&base)?;
        let target = base.join(&record.directory_name);
        reject_symlinks(&target)?;
        Ok(target)
    }
    pub fn archive_agent(
        &self,
        operation: &WorkspaceOperation,
        id: &str,
    ) -> Result<WorkspaceAgent, WorkspaceError> {
        self.validate_operation(operation)?;
        let mut c = self.conn()?;
        let c = c.as_mut().unwrap();
        let record = by_id(c, id)?.ok_or_else(|| error(K::Missing, "Agent record missing"))?;
        if record.category == AgentCategory::Archived {
            return Err(error(K::InvalidRequest, "Agent is already archived"));
        }
        let key = record.node_name.as_deref().unwrap_or(&record.runtime_name);
        if key != operation.key {
            return Err(error(
                K::InvalidRequest,
                "archive operation targets another node",
            ));
        }
        if read_intent(c, id)?.is_none() {
            let destination = loop {
                let name = random_id(c)?;
                let path = self.managed_directory().join("agents/archived").join(&name);
                if fs::symlink_metadata(&path).is_err() && !c.query_row("SELECT EXISTS(SELECT 1 FROM archive_intents WHERE destination_directory=?1)",[&name], |r| r.get::<_,bool>(0)).map_err(db_error)? { break name; }
            };
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            tx.execute(
                "INSERT INTO archive_intents VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    record.category.as_str(),
                    record.directory_name,
                    destination
                ],
            )
            .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        let intent = read_intent(c, id)?.unwrap();
        let moved = self.finish_archive(c, &intent, false);
        match moved {
            Ok(record) => {
                self.publish_committed(read_metadata(c)?, read_agents(c)?);
                Ok(record)
            }
            Err(e) => Err(e),
        }
    }
    /// Only resumes already persisted operations; it never migrates data.
    pub fn recover_archives(&self) -> Result<(), WorkspaceError> {
        let admission = lock(&self.admission)?;
        if !admission.nodes.is_empty() {
            return Err(error(
                K::Busy,
                "cannot recover archives while operations are active",
            ));
        }
        let mut c = self.conn()?;
        let c = c.as_mut().unwrap();
        let ids = {
            let mut q = c
                .prepare("SELECT agent_id FROM archive_intents ORDER BY agent_id")
                .map_err(db_error)?;
            let values = q
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?;
            values
        };
        for id in ids {
            let intent = read_intent(c, &id)?.unwrap();
            self.finish_archive(c, &intent, true)?;
        }
        self.publish_committed(read_metadata(c)?, read_agents(c)?);
        Ok(())
    }
    fn finish_archive(
        &self,
        c: &mut Connection,
        intent: &ArchiveIntent,
        recovering: bool,
    ) -> Result<WorkspaceAgent, WorkspaceError> {
        let fail_kind = if recovering {
            K::RecoveryFailed
        } else {
            K::ArchivePending
        };
        let old =
            by_id(c, &intent.id)?.ok_or_else(|| error(fail_kind, "archive record missing"))?;
        if old.category != intent.category || old.directory_name != intent.source {
            return Err(error(
                fail_kind,
                "archive intent disagrees with Agent record",
            ));
        }
        validate_segment(&intent.source)?;
        validate_segment(&intent.destination)?;
        if intent.source == intent.destination {
            return Err(error(
                fail_kind,
                "archive must allocate a new directory name",
            ));
        }
        let source = self.agent_directory(&old)?;
        let destination = self
            .managed_directory()
            .join("agents/archived")
            .join(&intent.destination);
        reject_symlinks(&destination)?;
        let source_exists = directory_exists(&source)?;
        let destination_exists = directory_exists(&destination)?;
        if source_exists == destination_exists {
            return Err(error(
                fail_kind,
                "archive requires exactly one of source or destination directory",
            )
            .at_phase("inspect_archive")
            .with_directory(self.managed_directory()));
        }
        if source_exists {
            if let Err(e) = fs::rename(&source, &destination) {
                // Only clear an intent when the filesystem confirms that no
                // move happened. Unknown outcomes must remain recoverable.
                if !recovering && directory_exists(&source)? && !directory_exists(&destination)? {
                    c.execute(
                        "DELETE FROM archive_intents WHERE agent_id=?1",
                        [&intent.id],
                    )
                    .map_err(db_error)?;
                    return Err(error(K::DatabaseFailed, e)
                        .at_phase("rename_archive")
                        .with_directory(source));
                }
                return Err(error(fail_kind, e)
                    .at_phase("rename_archive")
                    .with_directory(source));
            }
        }
        sync_dir(source.parent().unwrap())
            .and_then(|_| sync_dir(destination.parent().unwrap()))
            .map_err(|e| {
                error(fail_kind, e)
                    .at_phase("sync_archive")
                    .with_directory(&destination)
            })?;
        let mut archived = old.clone();
        archived.category = AgentCategory::Archived;
        archived.previous_node_name = old.node_name;
        archived.previous_alias = old.alias.clone();
        archived.node_name = None;
        archived.directory_name = intent.destination.clone();
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| error(fail_kind, e).at_phase("commit_archive"))?;
        tx.execute("UPDATE agents SET category='Archived',previous_node_name=node_name,previous_alias=alias,node_name=NULL,directory_name=?1 WHERE id=?2",params![intent.destination,intent.id]).map_err(|e|error(fail_kind,e).at_phase("commit_archive"))?;
        tx.execute(
            "DELETE FROM archive_intents WHERE agent_id=?1",
            [&intent.id],
        )
        .map_err(|e| error(fail_kind, e).at_phase("commit_archive"))?;
        tx.commit()
            .map_err(|e| error(fail_kind, e).at_phase("commit_archive"))?;
        Ok(archived)
    }
    /// Explicitly closes the shared connection. A Busy result still closes
    /// admission; callers must await idle, then retry close.
    pub fn close(&self) -> Result<(), WorkspaceError> {
        let mut admission = lock(&self.admission)?;
        admission.accepting = false;
        if !admission.nodes.is_empty() {
            return Err(error(K::Busy, "workspace operations remain in flight"));
        }
        let mut guard = lock(&self.connection)?;
        if let Some(conn) = guard.take() {
            if let Err((conn, e)) = conn.close() {
                *guard = Some(conn);
                return Err(error(K::CloseFailed, e));
            }
        }
        self.changes.send_replace(None);
        Ok(())
    }
    fn validate_operation(&self, operation: &WorkspaceOperation) -> Result<(), WorkspaceError> {
        if !Arc::ptr_eq(&self.connection, &operation.connection)
            || !Arc::ptr_eq(&self.admission, &operation.admission)
        {
            return Err(error(
                K::InvalidRequest,
                "operation belongs to another database",
            ));
        }
        if !lock(&self.admission)?.nodes.contains(&operation.key) {
            return Err(error(K::InvalidRequest, "operation is no longer admitted"));
        }
        Ok(())
    }
    fn conn(&self) -> Result<MutexGuard<'_, Option<Connection>>, WorkspaceError> {
        let c = lock(&self.connection)?;
        if c.is_none() {
            return Err(error(K::Closed, "workspace database is closed"));
        }
        Ok(c)
    }
    fn publish_committed(&self, metadata: WorkspaceMetadata, agents: Vec<WorkspaceAgent>) {
        self.changes.send_if_modified(|slot| {
            let Some(snapshot) = slot else {
                return false;
            };
            if snapshot.agents == agents && snapshot.metadata == metadata {
                return false;
            }
            snapshot.revision = snapshot
                .revision
                .checked_add(1)
                .expect("Workspace snapshot revision exhausted");
            snapshot.metadata = metadata;
            snapshot.agents = agents;
            true
        });
    }
}

fn read_metadata(c: &Connection) -> Result<WorkspaceMetadata, WorkspaceError> {
    let metadata = c
        .query_row(
            "SELECT address,alias,project_root FROM workspace WHERE singleton=1",
            [],
            |row| {
                Ok(WorkspaceMetadata {
                    address: row.get(0)?,
                    alias: row.get(1)?,
                    project_root: PathBuf::from(row.get::<_, String>(2)?),
                })
            },
        )
        .map_err(db_error)?;
    validate_metadata(&metadata)?;
    Ok(metadata)
}

#[derive(Debug)]
struct ArchiveIntent {
    id: String,
    category: AgentCategory,
    source: String,
    destination: String,
}
fn read_intent(c: &Connection, id: &str) -> Result<Option<ArchiveIntent>, WorkspaceError> {
    let value=c.query_row("SELECT agent_id,source_category,source_directory,destination_directory FROM archive_intents WHERE agent_id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional().map_err(db_error)?;
    value
        .map(|(id, category, source, destination)| {
            Ok(ArchiveIntent {
                id,
                category: parse_category(&category)?,
                source,
                destination,
            })
        })
        .transpose()
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceAgent> {
    let category: String = r.get(4)?;
    let category = match category.as_str() {
        "Normal" => AgentCategory::Normal,
        "Temporary" => AgentCategory::Temporary,
        "Archived" => AgentCategory::Archived,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let raw: String = r.get(8)?;
    let arguments = serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(WorkspaceAgent {
        id: r.get(0)?,
        parent_node: r.get(1)?,
        node_name: r.get(2)?,
        alias: r.get(3)?,
        category,
        directory_name: r.get(5)?,
        runtime_name: r.get(6)?,
        tool_path: PathBuf::from(r.get::<_, String>(7)?),
        tool_arguments: arguments,
        previous_node_name: r.get(9)?,
        previous_alias: r.get(10)?,
    })
}
fn by_id(c: &Connection, id: &str) -> Result<Option<WorkspaceAgent>, WorkspaceError> {
    c.query_row(&format!("{SELECT} WHERE id=?1"), [id], row)
        .optional()
        .map_err(db_error)
}
fn find_node(c: &Connection, node: &str) -> Result<Option<WorkspaceAgent>, WorkspaceError> {
    let field = if node.starts_with(TEMP_PREFIX) {
        "runtime_name"
    } else {
        "node_name"
    };
    c.query_row(
        &format!("{SELECT} WHERE {field}=?1 AND category!='Archived'"),
        [node],
        row,
    )
    .optional()
    .map_err(db_error)
}
fn read_agents(c: &Connection) -> Result<Vec<WorkspaceAgent>, WorkspaceError> {
    let mut q = c
        .prepare(&format!("{SELECT} ORDER BY id"))
        .map_err(db_error)?;
    let result = q
        .query_map([], row)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(result)
}
fn check_startable(c: &Connection, agent: &WorkspaceAgent) -> Result<(), WorkspaceError> {
    if agent.category == AgentCategory::Archived {
        return Err(error(K::InvalidRequest, "Agent is archived"));
    }
    if read_intent(c, &agent.id)?.is_some() {
        return Err(error(
            K::ArchivePending,
            "Agent has an unfinished archive intent",
        ));
    }
    Ok(())
}
fn random_id(c: &Connection) -> Result<String, WorkspaceError> {
    c.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .map_err(db_error)
}
fn parse_category(s: &str) -> Result<AgentCategory, WorkspaceError> {
    match s {
        "Normal" => Ok(AgentCategory::Normal),
        "Temporary" => Ok(AgentCategory::Temporary),
        "Archived" => Ok(AgentCategory::Archived),
        _ => Err(error(K::DatabaseFailed, "invalid Agent category")),
    }
}
fn connect(path: &Path) -> Result<Connection, WorkspaceError> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    c.busy_timeout(std::time::Duration::ZERO)
        .map_err(db_error)?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(db_error)?;
    Ok(c)
}
fn validate_schema(c: &Connection) -> Result<(), WorkspaceError> {
    let version: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(db_error)?;
    if version != 1 {
        return Err(error(
            K::DatabaseFailed,
            "unsupported Workspace schema; no automatic migration",
        ));
    }
    for (table, expected) in [
        (
            "workspace",
            vec!["singleton", "address", "alias", "project_root"],
        ),
        (
            "agents",
            vec![
                "id",
                "parent_node",
                "node_name",
                "alias",
                "category",
                "directory_name",
                "runtime_name",
                "tool_path",
                "tool_arguments",
                "previous_node_name",
                "previous_alias",
            ],
        ),
        (
            "archive_intents",
            vec![
                "agent_id",
                "source_category",
                "source_directory",
                "destination_directory",
            ],
        ),
    ] {
        let mut q = c
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(db_error)?;
        let columns = q
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        if columns != expected {
            return Err(error(K::DatabaseFailed, format!("invalid {table} schema")));
        }
    }
    let expected_schema = Connection::open_in_memory().map_err(db_error)?;
    expected_schema.execute_batch(SCHEMA).map_err(db_error)?;
    for table in ["workspace", "agents", "archive_intents"] {
        let sql = |connection: &Connection| -> Result<String, WorkspaceError> {
            connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .map_err(db_error)
        };
        let normalize = |sql: String| {
            sql.split_whitespace()
                .collect::<String>()
                .to_ascii_lowercase()
        };
        if normalize(sql(c)?) != normalize(sql(&expected_schema)?) {
            return Err(error(
                K::DatabaseFailed,
                format!("unsupported {table} schema constraints; no automatic migration"),
            ));
        }
    }
    let count: i64 = c
        .query_row("SELECT count(*) FROM workspace", [], |r| r.get(0))
        .map_err(db_error)?;
    if count != 1 {
        return Err(error(
            K::DatabaseFailed,
            "Workspace metadata must contain one row",
        ));
    }
    let violations: i64 = c
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .map_err(db_error)?;
    if violations != 0 {
        return Err(error(
            K::DatabaseFailed,
            "Workspace foreign key validation failed",
        ));
    }
    Ok(())
}
fn validate_metadata(m: &WorkspaceMetadata) -> Result<(), WorkspaceError> {
    let node = m
        .address
        .strip_prefix('/')
        .ok_or_else(|| error(K::InvalidRequest, "Workspace address must be absolute"))?;
    validate_node_name(node)?;
    validate_alias(Some(&m.alias))?;
    validate_absolute(&m.project_root)
}
fn validate_parent(parent: &str, workspace: &str) -> Result<(), WorkspaceError> {
    if parent.ends_with('/') {
        return Err(error(
            K::InvalidRequest,
            "parent node must not have a trailing slash",
        ));
    }
    let rest = if parent == workspace {
        ""
    } else {
        parent
            .strip_prefix(&format!("{workspace}/"))
            .ok_or_else(|| error(K::ParentMismatch, "parent is outside this Workspace"))?
    };
    if !rest.is_empty() {
        for s in rest.split('/') {
            validate_segment(s)?;
            if s.contains(':') {
                return Err(error(K::InvalidRequest, "parent must be a node path"));
            }
        }
    }
    Ok(())
}
pub(crate) fn validate_node_name(s: &str) -> Result<(), WorkspaceError> {
    validate_segment(s)?;
    if s.contains(':') || s.starts_with(TEMP_PREFIX) {
        return Err(error(K::InvalidRequest, "invalid or reserved node name"));
    }
    Ok(())
}
fn validate_operation_key(s: &str) -> Result<(), WorkspaceError> {
    if let Some(id) = s.strip_prefix(TEMP_PREFIX) {
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(
                K::InvalidRequest,
                "temporary key must contain 32 lowercase hex digits",
            ));
        }
        Ok(())
    } else {
        validate_node_name(s)
    }
}
fn validate_segment(s: &str) -> Result<(), WorkspaceError> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains(['/', '\\'])
        || s.chars().any(char::is_control)
    {
        return Err(error(K::InvalidRequest, "invalid managed path segment"));
    }
    Ok(())
}
fn validate_alias(s: Option<&str>) -> Result<(), WorkspaceError> {
    if s.is_some_and(|s| s.trim().is_empty() || s.chars().any(char::is_control)) {
        return Err(error(
            K::InvalidRequest,
            "alias must be nonempty, not all whitespace, and contain no control characters",
        ));
    }
    Ok(())
}
fn validate_absolute(p: &Path) -> Result<(), WorkspaceError> {
    if !p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || p.to_str().is_none_or(|s| s.chars().any(char::is_control))
    {
        return Err(error(
            K::InvalidRequest,
            "path must be a normalized absolute UTF-8 path",
        ));
    }
    Ok(())
}
fn validate_record(r: &WorkspaceAgent, workspace: &str) -> Result<(), WorkspaceError> {
    validate_parent(&r.parent_node, workspace)?;
    validate_absolute(&r.tool_path)?;
    validate_segment(&r.directory_name)?;
    validate_segment(&r.runtime_name)?;
    validate_alias(r.alias.as_deref())?;
    if r.id.len() != 32
        || !r
            .id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !r.tool_arguments.is_object()
    {
        return Err(error(K::InvalidRequest, "invalid record id or arguments"));
    }
    match r.category {
        AgentCategory::Normal => {
            let n = r
                .node_name
                .as_deref()
                .ok_or_else(|| error(K::InvalidRequest, "normal Agent has no node"))?;
            validate_node_name(n)?;
            if r.directory_name != n
                || r.runtime_name != n
                || r.previous_node_name.is_some()
                || r.previous_alias.is_some()
            {
                return Err(error(K::InvalidRequest, "normal Agent identity mismatch"));
            }
        }
        AgentCategory::Temporary => {
            if r.node_name.is_some()
                || r.directory_name != r.id
                || r.runtime_name != format!("{TEMP_PREFIX}{}", r.id)
                || r.previous_node_name.is_some()
                || r.previous_alias.is_some()
            {
                return Err(error(
                    K::InvalidRequest,
                    "temporary Agent identity mismatch",
                ));
            }
        }
        AgentCategory::Archived => {
            if r.node_name.is_some() {
                return Err(error(K::InvalidRequest, "archived Agent still owns node"));
            }
            if let Some(n) = &r.previous_node_name {
                validate_node_name(n)?;
                if r.runtime_name != *n {
                    return Err(error(K::InvalidRequest, "archived normal runtime mismatch"));
                }
            } else if r.runtime_name != format!("{TEMP_PREFIX}{}", r.id) {
                return Err(error(
                    K::InvalidRequest,
                    "archived temporary runtime mismatch",
                ));
            }
        }
    }
    Ok(())
}
fn checked_database_path(p: &Path, existing: bool) -> Result<PathBuf, WorkspaceError> {
    validate_absolute(p)?;
    if p.file_name().is_none_or(|n| n != "workspace.sqlite") {
        return Err(error(
            K::InvalidRequest,
            "Workspace database file must be workspace.sqlite",
        ));
    }
    reject_symlinks(p)?;
    let parent = p
        .parent()
        .ok_or_else(|| error(K::InvalidRequest, "database has no parent"))?;
    require_directory(parent)?;
    if existing && !p.is_file() {
        return Err(error(K::Missing, "Workspace database missing"));
    }
    Ok(fs::canonicalize(parent)
        .map_err(db_error)?
        .join("workspace.sqlite"))
}
fn validate_layout(root: &Path) -> Result<(), WorkspaceError> {
    for suffix in [
        "agents",
        "agents/normal",
        "agents/temporary",
        "agents/archived",
    ] {
        require_directory(&root.join(suffix))?;
    }
    Ok(())
}
fn reject_symlinks(p: &Path) -> Result<(), WorkspaceError> {
    let mut current = PathBuf::new();
    for segment in p.components() {
        current.push(segment.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(
                    error(K::InvalidRequest, "managed paths must not contain symlinks")
                        .with_directory(current),
                )
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(db_error(e)),
        }
    }
    Ok(())
}
fn require_directory(p: &Path) -> Result<(), WorkspaceError> {
    reject_symlinks(p)?;
    if !fs::metadata(p).map_err(db_error)?.is_dir() {
        return Err(error(K::InvalidRequest, "managed path must be a directory").with_directory(p));
    }
    Ok(())
}
fn directory_exists(p: &Path) -> Result<bool, WorkspaceError> {
    reject_symlinks(p)?;
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => Ok(true),
        Ok(_) => {
            Err(error(K::RecoveryFailed, "archive location is not a directory").with_directory(p))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(db_error(e)),
    }
}
fn sync_dir(p: &Path) -> std::io::Result<()> {
    File::open(p)?.sync_all()
}
fn path_text(p: &Path) -> Result<&str, WorkspaceError> {
    p.to_str()
        .ok_or_else(|| error(K::InvalidRequest, "path is not UTF-8"))
}
fn error(kind: K, message: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(kind, message)
}
fn db_error(e: impl std::fmt::Display) -> WorkspaceError {
    error(K::DatabaseFailed, e)
}
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, WorkspaceError> {
    mutex
        .lock()
        .map_err(|_| error(K::DatabaseFailed, "Workspace lock was poisoned"))
}

#[cfg(test)]
mod alias_admission_tests {
    use super::*;
    #[test]
    fn accepted_alias_write_blocks_close_and_idle_until_transaction_finishes() {
        let directory = tempfile::tempdir().unwrap();
        for kind in ["normal", "temporary", "archived"] {
            fs::create_dir_all(directory.path().join("agents").join(kind)).unwrap();
        }
        let db = WorkspaceDatabase::create(
            directory.path().join("workspace.sqlite"),
            WorkspaceMetadata {
                address: "/demo".into(),
                alias: "Demo".into(),
                project_root: "/project".into(),
            },
        )
        .unwrap();
        let connection = db.conn().unwrap();
        let worker = db.clone();
        let join = std::thread::spawn(move || worker.rename_workspace_alias("renamed".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while lock(&db.admission).unwrap().nodes.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "write did not acquire admission"
            );
            std::thread::yield_now();
        }
        let mut idle = Box::pin(db.wait_idle());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(idle.as_mut(), &mut context).is_pending());
        assert_eq!(db.close().unwrap_err().kind, K::Busy);
        drop(connection);
        join.join().unwrap().unwrap();
        assert!(std::future::Future::poll(idle.as_mut(), &mut context).is_ready());
        assert_eq!(db.metadata().unwrap().alias, "renamed");
        db.close().unwrap();
    }
}
