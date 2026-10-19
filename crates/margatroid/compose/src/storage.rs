use crate::{
    compile_str, normalize_path, open_regular, reject_symlinks, validate_logical_name,
    MAX_WORKSPACE_YAML_BYTES,
};
use margatroid_types::workspace_catalog::{
    WorkspaceCatalogEntry, WorkspaceCatalogStore, WorkspaceDeletion,
};
use margatroid_types::workspace_components::{
    WorkspaceAgentDefinition, WorkspaceDefinition, WorkspaceError, WorkspaceErrorKind,
};
use resource_id_plugin::ResourceID;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

#[path = "operations.rs"]
mod operations;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Internal catalog subdirectory holding removed workspaces and in-flight moves.
/// It is never listed and cannot be a workspace name.
const INTERNAL_DIRECTORY: &str = ".trash";

/// Blocking local storage. All operations hold the same per-root advisory file lock.
/// Lifecycle additionally owns the lease covering state checks and mutations.
pub struct FileWorkspaceCatalogStore {
    root: PathBuf,
    lock: Mutex<()>,
    recovery_error: Mutex<Option<String>>,
}

fn invalid(message: impl ToString) -> WorkspaceError {
    WorkspaceError::new(WorkspaceErrorKind::InvalidDefinition, message.to_string())
}
fn conflict() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::WorkspaceBusy,
        "workspace source revision changed",
    )
}
fn exists() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::DuplicateWorkspace,
        "workspace name is already in use",
    )
}
fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn workspace_id(name: &str) -> Result<ResourceID, WorkspaceError> {
    ResourceID::new("workspace", "local", name, None::<String>).map_err(invalid)
}
fn unique_suffix() -> String {
    format!(
        "{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

impl FileWorkspaceCatalogStore {
    /// `root` must be an absolute canonical path ending in `workspaces`.
    /// Missing directories are created; existing symlinks are rejected.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = normalize_path(root.as_ref().to_path_buf()).map_err(invalid)?;
        if root.file_name().and_then(|v| v.to_str()) != Some("workspaces") {
            return Err(invalid("catalog root must end in workspaces"));
        }
        reject_symlinks(&root).map_err(invalid)?;
        fs::create_dir_all(&root).map_err(invalid)?;
        reject_symlinks(&root).map_err(invalid)?;
        if fs::canonicalize(&root).map_err(invalid)? != root {
            return Err(invalid("catalog root is not canonical"));
        }
        let store = Self {
            root,
            lock: Mutex::new(()),
            recovery_error: Mutex::new(None),
        };
        if let Err(error) = store.locked(|| operations::recover(&store.root)) {
            *store
                .recovery_error
                .lock()
                .map_err(|_| invalid("recovery lock poisoned"))? =
                Some(format!("recovery_required: {error}"));
        }
        Ok(store)
    }

    pub fn default_root() -> Result<PathBuf, WorkspaceError> {
        let home = std::env::var_os("HOME").ok_or_else(|| invalid("HOME is not configured"))?;
        let home = fs::canonicalize(PathBuf::from(home)).map_err(invalid)?;
        Ok(home.join(".margatroid").join("workspaces"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Internal directory holding removed workspaces; never created by listing.
    pub fn trash_root(&self) -> PathBuf {
        self.root.join(INTERNAL_DIRECTORY)
    }

    fn locked<T>(
        &self,
        operation: impl FnOnce() -> Result<T, WorkspaceError>,
    ) -> Result<T, WorkspaceError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("catalog lock poisoned"))?;
        reject_symlinks(&self.root).map_err(invalid)?;
        let path = self.root.join(".catalog.lock");
        reject_symlinks(&path).map_err(invalid)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .mode(0o600);
        }
        let file = options.open(path).map_err(invalid)?;
        if !file.metadata().map_err(invalid)?.is_file() {
            return Err(invalid("catalog lock must be a regular file"));
        }
        file.lock().map_err(invalid)?;
        operation()
    }

    fn available(&self) -> Result<(), WorkspaceError> {
        if let Some(error) = self
            .recovery_error
            .lock()
            .map_err(|_| invalid("recovery lock poisoned"))?
            .as_ref()
        {
            return Err(invalid(error));
        }
        operations::recover(&self.root).map_err(|error| {
            let message = format!("recovery_required: {error}");
            if let Ok(mut state) = self.recovery_error.lock() {
                *state = Some(message.clone());
            }
            invalid(message)
        })
    }

    fn directory_for(&self, name: &str) -> Result<PathBuf, WorkspaceError> {
        self.available()?;
        validate_logical_name(name, "workspace").map_err(invalid)?;
        if name.starts_with('.') {
            return Err(invalid("workspace name is reserved"));
        }
        workspace_id(name)?;
        Ok(self.root.join(name))
    }

    fn path_for(&self, id: &ResourceID) -> Result<PathBuf, WorkspaceError> {
        self.available()?;
        self.directory_for(id.name())?;
        validate_logical_name(id.name(), "workspace").map_err(invalid)?;
        if *id != workspace_id(id.name())? {
            return Err(invalid("expected canonical local workspace ID"));
        }
        Ok(self.root.join(id.name()).join("workspace.yaml"))
    }

    fn entry(&self, name: String, path: PathBuf) -> WorkspaceCatalogEntry {
        let recovery_error = self
            .recovery_error
            .lock()
            .ok()
            .and_then(|state| state.clone());
        let mut error = validate_logical_name(&name, "workspace")
            .err()
            .map(|e| e.to_string());
        let id = match workspace_id(&name) {
            Ok(id) => id,
            Err(e) => {
                error = Some(e.to_string());
                workspace_id(&format!("invalid-{}", revision(name.as_bytes())))
                    .expect("hash is a safe name")
            }
        };
        let mut entry = WorkspaceCatalogEntry {
            id,
            name,
            path,
            revision: String::new(),
            definition: None,
            error: recovery_error.or(error),
        };
        if let Err(e) = reject_symlinks(&entry.path) {
            entry.error = Some(e.to_string());
            return entry;
        }
        match source_bytes(&entry.path) {
            Ok(bytes) => {
                entry.revision = revision(&bytes);
                if entry.error.is_none() {
                    match String::from_utf8(bytes)
                        .map_err(invalid)
                        .and_then(|source| compile_str(&source, &entry.path).map_err(invalid))
                    {
                        Ok(definition) => entry.definition = Some(definition),
                        Err(error) => entry.error = Some(error.to_string()),
                    }
                }
            }
            Err(error) => entry.error = Some(error.to_string()),
        }
        entry
    }

    fn load_inner(
        &self,
        id: &ResourceID,
        expected: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        let path = self.path_for(id)?;
        let entry = self.entry(id.name().to_owned(), path);
        if entry.revision != expected {
            return Err(conflict());
        }
        Ok(entry)
    }

    /// Compile the source that a rename must install at `workspace.yaml`.
    /// The supplied definition is authoritative for `home_root` and must agree with the
    /// document apart from workspace and member addresses, which are re-derived.
    fn renamed_source(
        &self,
        path: &Path,
        definition: &WorkspaceDefinition,
        current: &WorkspaceDefinition,
        new_name: &str,
    ) -> Result<(String, WorkspaceDefinition), WorkspaceError> {
        if definition.id != current.id
            || definition.name != current.name
            || definition.project_root != current.project_root
            || definition.agents.len() != current.agents.len()
        {
            return Err(invalid(
                "rename definition does not match the stored workspace",
            ));
        }
        let mut agents = Vec::with_capacity(definition.agents.len());
        for (supplied, existing) in definition.agents.iter().zip(&current.agents) {
            if supplied.name != existing.name
                || supplied.driver != existing.driver
                || supplied.home_root != existing.home_root
            {
                return Err(invalid(
                    "rename definition does not match the stored workspace",
                ));
            }
            let id = ResourceID::new("agent", new_name, &supplied.name, None::<String>)
                .map_err(invalid)?;
            if id.name() != existing.id.name() || supplied.id.name() != &supplied.name {
                return Err(invalid(
                    "rename definition does not match the stored workspace",
                ));
            }
            agents.push(WorkspaceAgentDefinition {
                id,
                name: supplied.name.clone(),
                driver: supplied.driver.clone(),
                home_root: supplied.home_root.clone(),
                history_path: path
                    .parent()
                    .expect("managed path has parent")
                    .join("agents")
                    .join(&supplied.name)
                    .join("state.sqlite"),
            });
        }
        let source = serde_yaml::to_string(&SavedWorkspace {
            name: new_name,
            project_root: &definition.project_root,
            agents: agents
                .iter()
                .map(|agent| SavedAgent {
                    name: &agent.name,
                    driver: agent.driver.to_string(),
                    home_root: &agent.home_root,
                })
                .collect(),
        })
        .map_err(invalid)?;
        let compiled = compile_candidate(&source, path)?;
        if compiled.name != new_name || compiled.id != workspace_id(new_name)? {
            return Err(invalid("renamed definition is inconsistent"));
        }
        for (position, (actual, supplied)) in compiled.agents.iter().zip(&agents).enumerate() {
            if actual.id != supplied.id
                || actual.home_root != supplied.home_root
                || actual.history_path != supplied.history_path
            {
                return Err(invalid(format!(
                    "member {} does not match managed layout",
                    position
                )));
            }
        }
        Ok((source, compiled))
    }

    /// Validate the rename request without touching disk, then return the rewritten source.
    fn prepare_rename(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        new_name: &str,
        definition: &WorkspaceDefinition,
    ) -> Result<(PathBuf, PathBuf, String), WorkspaceError> {
        let old = self.path_for(id)?;
        let new = self.directory_for(new_name)?.join("workspace.yaml");
        let entry = self.entry(id.name().to_owned(), old.clone());
        if entry.revision != expected_revision {
            return Err(conflict());
        }
        let current = entry
            .definition
            .clone()
            .filter(|_| entry.error.is_none())
            .ok_or_else(|| {
                invalid(
                    entry
                        .error
                        .clone()
                        .unwrap_or_else(|| "workspace definition unavailable".into()),
                )
            })?;
        if definition != &current {
            return Err(invalid(
                "rename definition does not match the stored workspace",
            ));
        }
        if new_name == id.name() {
            return Err(invalid("workspace name is unchanged"));
        }
        if new
            .parent()
            .expect("managed path")
            .try_exists()
            .map_err(invalid)?
        {
            return Err(exists());
        }
        reject_symlinks(&new).map_err(invalid)?;
        let (source, _) = self.renamed_source(&new, definition, &current, new_name)?;
        Ok((old, new, source))
    }
}

/// Compile a candidate document whose destination directory does not exist yet.
/// Only that missing directory is tolerated; every other compile failure is a rejection.
fn compile_candidate(source: &str, path: &Path) -> Result<WorkspaceDefinition, WorkspaceError> {
    compile_str(source, path).map_err(|error| {
        let missing = error.message().contains("must exist")
            && path.parent().is_some_and(|parent| !parent.exists());
        if missing {
            invalid("renamed workspace home does not exist yet")
        } else {
            invalid(error)
        }
    })
}

fn source_bytes(path: &Path) -> Result<Vec<u8>, WorkspaceError> {
    reject_symlinks(path).map_err(invalid)?;
    let file = open_regular(path).map_err(invalid)?;
    let mut bytes = Vec::new();
    file.take(MAX_WORKSPACE_YAML_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(invalid)?;
    if bytes.len() > MAX_WORKSPACE_YAML_BYTES {
        return Err(invalid("workspace YAML exceeds size limit"));
    }
    Ok(bytes)
}

fn sync_directory(path: &Path) -> Result<(), WorkspaceError> {
    File::open(path).and_then(|f| f.sync_all()).map_err(invalid)
}

#[derive(Serialize)]
struct SavedWorkspace<'a> {
    name: &'a str,
    project_root: &'a Path,
    agents: Vec<SavedAgent<'a>>,
}
#[derive(Serialize)]
struct SavedAgent<'a> {
    name: &'a str,
    driver: String,
    home_root: &'a Path,
}

impl WorkspaceCatalogStore for FileWorkspaceCatalogStore {
    fn list(&self) -> Result<Vec<WorkspaceCatalogEntry>, WorkspaceError> {
        self.locked(|| {
            let _ = self.available();
            let mut entries = Vec::new();
            for item in fs::read_dir(&self.root).map_err(invalid)? {
                let item = item.map_err(invalid)?;
                let kind = item.file_type().map_err(invalid)?;
                if kind.is_dir() || kind.is_symlink() {
                    let name = item.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    entries.push(self.entry(name, item.path().join("workspace.yaml")));
                }
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(entries)
        })
    }

    fn load(
        &self,
        id: &ResourceID,
        expected_revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        self.locked(|| self.load_inner(id, expected_revision))
    }

    fn save(
        &self,
        definition: &WorkspaceDefinition,
        expected_revision: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        self.locked(|| {
            let path = self.path_for(&definition.id)?;
            if definition.name != definition.id.name() {
                return Err(invalid("workspace ID and name disagree"));
            }
            let source = serde_yaml::to_string(&SavedWorkspace {
                name: &definition.name,
                project_root: &definition.project_root,
                agents: definition
                    .agents
                    .iter()
                    .map(|agent| SavedAgent {
                        name: &agent.name,
                        driver: agent.driver.to_string(),
                        home_root: &agent.home_root,
                    })
                    .collect(),
            })
            .map_err(invalid)?;
            let compiled = compile_str(&source, &path).map_err(invalid)?;
            for (actual, supplied) in compiled.agents.iter().zip(&definition.agents) {
                if actual.id != supplied.id
                    || actual.history_path != supplied.history_path
                    || actual.home_root != supplied.home_root
                {
                    return Err(invalid(
                        "agent identity or data paths do not match managed layout",
                    ));
                }
            }
            reject_symlinks(&path).map_err(invalid)?;
            let before = match source_bytes(&path) {
                Ok(bytes) => Some(bytes),
                Err(_) if !path.try_exists().map_err(invalid)? => None,
                Err(error) => return Err(error),
            };
            let before_revision = before.as_deref().map(revision).unwrap_or_default();
            if before_revision != expected_revision {
                return Err(conflict());
            }
            let directory = path.parent().expect("managed path has parent");
            fs::create_dir_all(directory).map_err(invalid)?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(invalid)?;
            reject_symlinks(&path).map_err(invalid)?;
            let temp = directory.join(format!(
                ".workspace-{}-{}.tmp",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut temp_created = false;
            let result = (|| {
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temp).map_err(invalid)?;
                temp_created = true;
                file.write_all(source.as_bytes()).map_err(invalid)?;
                file.sync_all().map_err(invalid)?;
                reject_symlinks(&path).map_err(invalid)?;
                let current = match source_bytes(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(_) if !path.try_exists().map_err(invalid)? => None,
                    Err(error) => return Err(error),
                };
                if current != before {
                    return Err(conflict());
                }
                if before.is_none() {
                    fs::hard_link(&temp, &path).map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            conflict()
                        } else {
                            invalid(error)
                        }
                    })?;
                    fs::remove_file(&temp).map_err(invalid)?;
                } else {
                    fs::rename(&temp, &path).map_err(invalid)?;
                }
                File::open(directory)
                    .and_then(|f| f.sync_all())
                    .map_err(invalid)?;
                Ok(())
            })();
            if result.is_err() && temp_created {
                let _ = fs::remove_file(&temp);
            }
            result?;
            Ok(WorkspaceCatalogEntry {
                id: compiled.id.clone(),
                name: compiled.name.clone(),
                path,
                revision: revision(source.as_bytes()),
                definition: Some(compiled),
                error: None,
            })
        })
    }

    fn rename(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        new_name: &str,
        definition: &WorkspaceDefinition,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        self.locked(|| {
            let (_old_path, new_path, source) =
                self.prepare_rename(id, expected_revision, new_name, definition)?;
            operations::migrate(self, id.name(), new_name, expected_revision, &source, None)?;
            let entry = self.entry(new_name.to_owned(), new_path);
            if entry.revision != revision(source.as_bytes()) {
                return Err(invalid(
                    "renamed workspace source is not the written document",
                ));
            }
            Ok(entry)
        })
    }

    fn rename_member(
        &self,
        id: &ResourceID,
        expected_revision: &str,
        agent: &ResourceID,
        new_name: &str,
    ) -> Result<WorkspaceCatalogEntry, WorkspaceError> {
        self.locked(|| {
            let path = self.path_for(id)?;
            let entry = self.load_inner(id, expected_revision)?;
            let mut definition = entry
                .definition
                .filter(|_| entry.error.is_none())
                .ok_or_else(|| invalid("workspace definition unavailable"))?;
            validate_logical_name(new_name, "agent").map_err(invalid)?;
            let position = definition
                .agents
                .iter()
                .position(|member| &member.id == agent)
                .ok_or_else(|| invalid("member does not belong to workspace"))?;
            if agent.name() == new_name
                || definition
                    .agents
                    .iter()
                    .any(|member| member.name == new_name)
                || self
                    .root
                    .join(id.name())
                    .join("agents")
                    .join(new_name)
                    .exists()
            {
                return Err(invalid("target member name or retained data conflicts"));
            }
            definition.agents[position].name = new_name.into();
            let source = serde_yaml::to_string(&SavedWorkspace {
                name: &definition.name,
                project_root: &definition.project_root,
                agents: definition
                    .agents
                    .iter()
                    .map(|member| SavedAgent {
                        name: &member.name,
                        driver: member.driver.to_string(),
                        home_root: &member.home_root,
                    })
                    .collect(),
            })
            .map_err(invalid)?;
            compile_candidate(&source, &path)?;
            operations::migrate(
                self,
                id.name(),
                id.name(),
                expected_revision,
                &source,
                Some((agent.name(), new_name)),
            )?;
            Ok(self.entry(id.name().into(), path))
        })
    }

    fn delete(
        &self,
        id: &ResourceID,
        expected_revision: &str,
    ) -> Result<WorkspaceDeletion, WorkspaceError> {
        self.locked(|| {
            let path = self.path_for(id)?;
            let directory = path
                .parent()
                .expect("managed path has parent")
                .to_path_buf();
            let entry = self.entry(id.name().to_owned(), path);
            if entry.revision != expected_revision || !directory.exists() {
                return Err(conflict());
            }
            let removed = operations::delete(self, id.name(), expected_revision)?;
            Ok(WorkspaceDeletion {
                id: entry.id,
                path: directory,
                removed_path: removed,
            })
        })
    }
}
