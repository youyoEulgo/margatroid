use margatroid_types::resource_events::{ResourceLoadError, ResourceLoadErrorKind as Kind};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub(crate) fn error(kind: Kind, message: impl Into<String>) -> ResourceLoadError {
    ResourceLoadError {
        kind,
        message: message.into(),
    }
}
pub(crate) fn normalize_root(root: PathBuf) -> Result<PathBuf, ResourceLoadError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(error(
            Kind::InvalidRoot,
            format!(
                "root {} must be absolute without parent traversal",
                root.display()
            ),
        ));
    }
    Ok(root)
}
fn io_error(e: std::io::Error) -> ResourceLoadError {
    error(
        if e.kind() == std::io::ErrorKind::NotFound {
            Kind::NotFound
        } else {
            Kind::ReadFailed
        },
        e.to_string(),
    )
}
pub(crate) fn checked_path(path: &Path, directory: bool) -> Result<(), ResourceLoadError> {
    checked_path_inner(path, directory)
        .map_err(|e| context(format!("attempted path {}", path.display()), e))
}
fn checked_path_inner(path: &Path, directory: bool) -> Result<(), ResourceLoadError> {
    let mut current = PathBuf::new();
    let parts: Vec<_> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        if matches!(part, Component::ParentDir) {
            return Err(error(Kind::InvalidRoot, "parent traversal"));
        }
        current.push(part.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|e| context(format!("inspect {}", current.display()), io_error(e)))?;
        if metadata.file_type().is_symlink() {
            return Err(error(
                Kind::SymlinkNotAllowed,
                format!("symlink path component {}", current.display()),
            ));
        }
        let is_directory = index + 1 < parts.len() || directory;
        if (is_directory && !metadata.is_dir()) || (!is_directory && !metadata.is_file()) {
            return Err(error(
                Kind::InvalidFileType,
                format!(
                    "expected directory or ordinary file at {}",
                    current.display()
                ),
            ));
        }
    }
    Ok(())
}
pub(crate) fn read_text(
    path: &Path,
    maximum: u64,
    cancelled: impl Fn() -> bool,
) -> Result<String, ResourceLoadError> {
    read_text_inner(path, maximum, cancelled)
        .map_err(|e| context(format!("read {}", path.display()), e))
}
fn read_text_inner(
    path: &Path,
    maximum: u64,
    cancelled: impl Fn() -> bool,
) -> Result<String, ResourceLoadError> {
    if cancelled() {
        return Err(error(Kind::Cancelled, "load cancelled"));
    }
    checked_path(path, false)?;
    let mut file = File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > maximum {
        return Err(error(Kind::LimitExceeded, "file exceeds byte limit"));
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if cancelled() {
            return Err(error(Kind::Cancelled, "load cancelled"));
        }
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if (bytes.len() as u64).saturating_add(count as u64) > maximum {
            return Err(error(Kind::LimitExceeded, "file exceeds byte limit"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let text = String::from_utf8(bytes).map_err(|e| error(Kind::DecodeFailed, e.to_string()))?;
    if text.trim().is_empty() {
        return Err(error(Kind::InvalidDefinition, "empty file"));
    }
    if cancelled() {
        return Err(error(Kind::Cancelled, "load cancelled"));
    }
    Ok(text)
}

pub(crate) fn context(
    context: impl std::fmt::Display,
    source: ResourceLoadError,
) -> ResourceLoadError {
    error(source.kind, format!("{context}: {}", source.message))
}
