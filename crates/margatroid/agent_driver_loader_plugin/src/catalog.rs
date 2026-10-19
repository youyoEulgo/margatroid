use crate::types::AgentDriverLoadLimits;
use margatroid_types::agent_driver_events::DriverLoadError;
use margatroid_types::discovery::{AgentDriverCatalogSnapshot, DiscoveryError};
use resource_id_plugin::ResourceID;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum entries inspected in any directory and maximum packages validated per scan.
pub const MAX_CATALOG_DIRECTORY_ENTRIES: usize = 4096;

fn error(message: impl Into<String>) -> DriverLoadError {
    DriverLoadError::new(message)
}

fn list_directories(path: &Path) -> Result<Vec<String>, DriverLoadError> {
    let listing =
        fs::read_dir(path).map_err(|e| error(format!("cannot list {}: {e}", path.display())))?;
    let mut names = Vec::new();
    for (index, entry) in listing.enumerate() {
        if index >= MAX_CATALOG_DIRECTORY_ENTRIES {
            return Err(error(format!(
                "directory {} exceeds {MAX_CATALOG_DIRECTORY_ENTRIES} entries",
                path.display()
            )));
        }
        let entry = entry.map_err(|e| error(format!("cannot inspect {}: {e}", path.display())))?;
        if entry
            .file_type()
            .map_err(|e| error(e.to_string()))?
            .is_dir()
        {
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
    }
    names.sort();
    Ok(names)
}

fn candidates(root: &Path) -> Result<(Vec<ResourceID>, Vec<DiscoveryError>), DriverLoadError> {
    let root = crate::handler::normalize_root(root.to_path_buf())?;
    let mut checked = PathBuf::new();
    for part in root.components() {
        checked.push(part);
        let metadata = fs::symlink_metadata(&checked).map_err(|e| {
            error(format!(
                "cannot inspect driver root {}: {e}",
                checked.display()
            ))
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(error(format!(
                "driver root {} must contain only plain directories",
                checked.display()
            )));
        }
    }
    let mut ids = Vec::new();
    let mut errors = Vec::new();
    for scope in list_directories(&root)? {
        let scope_path = root.join(&scope);
        let names = match list_directories(&scope_path) {
            Ok(names) => names,
            Err(e) => {
                errors.push(DiscoveryError {
                    path: scope_path,
                    message: e.to_string(),
                });
                continue;
            }
        };
        for name in names {
            let path = scope_path.join(&name);
            let tags = match list_directories(&path) {
                Ok(tags) => tags,
                Err(e) => {
                    errors.push(DiscoveryError {
                        path,
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            for tag in tags {
                let Ok(id) = ResourceID::new("driver", scope.clone(), name.clone(), Some(tag))
                else {
                    continue;
                };
                if ids.len() >= MAX_CATALOG_DIRECTORY_ENTRIES {
                    return Err(error(format!(
                        "driver root {} exceeds {MAX_CATALOG_DIRECTORY_ENTRIES} packages",
                        root.display()
                    )));
                }
                ids.push(id);
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok((ids, errors))
}

/// Validates installed packages without mounting resources or executing scripts.
pub async fn scan_validated(
    root: &Path,
    limits: &AgentDriverLoadLimits,
) -> Result<AgentDriverCatalogSnapshot, DriverLoadError> {
    let owned_root = root.to_path_buf();
    let (candidates, mut errors) = tokio::task::spawn_blocking(move || candidates(&owned_root))
        .await
        .map_err(|e| error(format!("driver catalog scan worker failed: {e}")))??;
    let mut drivers = Vec::new();
    for id in candidates {
        match crate::handler::read_driver_files(root, &id, limits).await {
            Ok(_) => drivers.push(id),
            Err(e) => errors.push(DiscoveryError {
                path: root.join(id.scope()).join(id.name()).join(id.tag()),
                message: e.to_string(),
            }),
        }
    }
    errors.sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
    Ok(AgentDriverCatalogSnapshot { drivers, errors })
}
