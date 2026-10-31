mod handler;
mod system;
mod types;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimeHandle;
use closure_plugin::WorldClosureExt;
use core_plugin::{App, Plugin, Resource};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
use workspace_core::{
    load_workspace_definition, WorkspaceDefinitions, WorkspaceError, WorkspaceErrorKind as K,
};

pub struct WorkspacePlugin {
    pub schedule: String,
    definitions: WorkspaceDefinitions,
    root: Option<PathBuf>,
    file: Option<PathBuf>,
}
impl Default for WorkspacePlugin {
    fn default() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.into(),
            definitions: WorkspaceDefinitions::default(),
            root: None,
            file: None,
        }
    }
}
impl WorkspacePlugin {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().map_err(invalid)?.join(root)
        };
        reject_symlinks(&root)?;
        let root = std::fs::canonicalize(root).map_err(invalid)?;
        let metadata = std::fs::metadata(&root).map_err(invalid)?;
        let (directory, file, paths) = if metadata.is_file() {
            (
                root.parent()
                    .ok_or_else(|| invalid("definition has no parent"))?
                    .to_path_buf(),
                Some(root.clone()),
                vec![root],
            )
        } else if metadata.is_dir() {
            let mut paths = Vec::new();
            let mut entries = std::fs::read_dir(&root)
                .map_err(invalid)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(invalid)?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let kind = entry.file_type().map_err(invalid)?;
                if kind.is_symlink() {
                    return Err(invalid("workspace root contains a symbolic link"));
                }
                if kind.is_dir() {
                    let path = entry.path().join("workspace.yaml");
                    if path.exists() {
                        reject_symlinks(&path)?;
                        paths.push(path);
                    }
                }
            }
            (root, None, paths)
        } else {
            return Err(invalid("workspace root must be a directory or YAML file"));
        };
        if paths.len() > 4096 {
            return Err(invalid("workspace definition count exceeds 4096"));
        }
        let mut entries = BTreeMap::new();
        for path in paths {
            let definition = load_workspace_definition(&path)?;
            validate_definition_names(&definition)?;
            entries.insert(path, definition);
        }
        Ok(Self {
            definitions: WorkspaceDefinitions {
                entries: std::sync::Arc::new(std::sync::RwLock::new(entries)),
            },
            root: Some(directory),
            file,
            ..Self::default()
        })
    }
}
#[derive(Clone)]
pub(crate) struct WorkspaceInstalled {
    pub schedule: String,
    pub definitions: WorkspaceDefinitions,
    pub root: Option<PathBuf>,
    pub file: Option<PathBuf>,
}
impl Resource for WorkspaceInstalled {}
impl Plugin for WorkspacePlugin {
    fn build(self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<WorkspaceInstalled>(),
            "WorkspacePlugin is already installed"
        );
        assert!(
            app.contains_schedule(&self.schedule)
                && app.world().contains_resource::<RuntimeHandle>()
                && app.world().contains_resource::<AsyncRuntimeHandle>(),
            "WorkspacePlugin requires schedule, RuntimePlugin and AsyncRuntimePlugin"
        );
        let _ = app.world().closure_sender(&self.schedule);
        app.world_mut().insert_resource(self.definitions.clone());
        app.world_mut().insert_resource(WorkspaceInstalled {
            schedule: self.schedule.clone(),
            definitions: self.definitions,
            root: self.root,
            file: self.file,
        });
        app.add_system(&self.schedule, system::stop_workspace_system)
            .add_system(&self.schedule, system::restart_workspace_system)
            .add_system(&self.schedule, system::start_workspace_system);
    }
}
pub(crate) fn invalid(error: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::new(K::InvalidDefinition, error)
}
pub(crate) fn validate_name(name: &str) -> Result<(), WorkspaceError> {
    if name.trim().is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(invalid("invalid workspace or member name"));
    }
    Ok(())
}
pub(crate) fn validate_definition_names(
    definition: &workspace_core::WorkspaceDefinition,
) -> Result<(), WorkspaceError> {
    validate_name(&definition.name)?;
    for member in &definition.members {
        validate_name(&member.name)?;
    }
    Ok(())
}
pub(crate) fn checked_path(
    installed: &WorkspaceInstalled,
    path: &Path,
) -> Result<PathBuf, WorkspaceError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
        || path.file_name().and_then(|n| n.to_str()) != Some("workspace.yaml")
        || path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return Err(WorkspaceError::new(
            K::InvalidRequest,
            "expected an absolute workspace.yaml path without traversal",
        ));
    }
    if let Some(file) = &installed.file {
        if path != file {
            return Err(WorkspaceError::new(
                K::InvalidRequest,
                "workspace path is outside configured definition",
            ));
        }
    } else if let Some(root) = &installed.root {
        let relative = path.strip_prefix(root).map_err(|_| {
            WorkspaceError::new(
                K::InvalidRequest,
                "workspace path is outside configured root",
            )
        })?;
        if relative.components().count() != 2 {
            return Err(WorkspaceError::new(
                K::InvalidRequest,
                "expected root/<workspace>/workspace.yaml",
            ));
        }
    }
    Ok(path.to_path_buf())
}
pub(crate) fn reject_symlinks(path: &Path) -> Result<(), WorkspaceError> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid(
                    "symbolic links are not permitted in managed workspace paths",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(invalid(error)),
        }
    }
    Ok(())
}
