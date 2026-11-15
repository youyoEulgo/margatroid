mod error;
pub mod types;
mod ui;
pub use ui::{
    DriverUiBundle, DriverUiError, DriverUiFile, DriverUiMonitor, DriverUiReadLimits,
    DriverUiState, DriverUiWatch, open_driver_ui_watch, read_driver_ui, run_driver_ui_monitor,
};

pub use error::AgentDriverError;
pub use types::{AgentDriverInput, AgentDriverReadLimits};

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

pub fn read_driver(
    manifest_path: &Path,
    limits: &AgentDriverReadLimits,
) -> Result<AgentDriverInput, AgentDriverError> {
    if manifest_path.file_name().is_none() {
        return Err(AgentDriverError::InvalidPath);
    }
    let origin = manifest_path.to_path_buf();
    let program = read_text(&origin, limits.program_bytes)?;
    Ok(AgentDriverInput { program, origin })
}

pub fn read_initial_settings(path: &Path, limit: usize) -> Result<String, AgentDriverError> {
    let text = read_text(path, limit)?;
    toml::from_str::<toml::Table>(&text).map_err(|_| AgentDriverError::InvalidDefaultSettings)?;
    Ok(text)
}

fn read_text(path: &Path, limit: usize) -> Result<String, AgentDriverError> {
    let metadata = fs::metadata(path).map_err(|error| read_error(path, error))?;
    if !metadata.is_file() {
        return Err(AgentDriverError::InvalidPath);
    }
    let maximum = u64::try_from(limit).unwrap_or(u64::MAX);
    if metadata.len() > maximum {
        return Err(AgentDriverError::TooLarge);
    }
    let file = File::open(path).map_err(|error| read_error(path, error))?;
    if !file
        .metadata()
        .map_err(|error| read_error(path, error))?
        .is_file()
    {
        return Err(AgentDriverError::InvalidPath);
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| read_error(path, error))?;
    if bytes.len() > limit {
        return Err(AgentDriverError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| AgentDriverError::InvalidUtf8)
}

fn read_error(path: &Path, error: io::Error) -> AgentDriverError {
    match error.kind() {
        io::ErrorKind::NotFound => AgentDriverError::FileMissing,
        io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory => AgentDriverError::InvalidPath,
        _ => AgentDriverError::ReadFailed(format!("{}: {error}", path.display())),
    }
}
