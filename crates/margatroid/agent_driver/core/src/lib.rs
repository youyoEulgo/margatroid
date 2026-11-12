mod error;
pub mod types;
mod ui;
pub use ui::{
    DriverUiBundle, DriverUiError, DriverUiFile, DriverUiMonitor, DriverUiReadLimits,
    DriverUiState, DriverUiWatch, open_driver_ui_watch, read_driver_ui, run_driver_ui_monitor,
};

pub use error::AgentDriverError;
pub use types::{
    AgentDriverInput, AgentDriverManifest, AgentDriverReadLimits, DriverInferenceConfig,
};

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
    let directory = manifest_path
        .parent()
        .ok_or(AgentDriverError::InvalidPath)?;
    let source = read_text(manifest_path, limits.manifest_bytes)?;
    let manifest: AgentDriverManifest =
        toml::from_str(&source).map_err(|_| AgentDriverError::InvalidManifest)?;
    validate_manifest(&manifest)?;
    let origin = directory.join("base.lua");
    let program = read_text(&origin, limits.program_bytes)?;
    let default_settings = read_text(&directory.join("setting.toml"), limits.settings_bytes)?;
    toml::from_str::<toml::Table>(&default_settings)
        .map_err(|_| AgentDriverError::InvalidDefaultSettings)?;
    Ok(AgentDriverInput {
        manifest,
        program,
        origin,
        default_settings,
    })
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

fn validate_manifest(manifest: &AgentDriverManifest) -> Result<(), AgentDriverError> {
    let model = &manifest.inference;
    if manifest.schema_version != 1
        || model.model.trim().is_empty()
        || model.model.len() > 1024
        || model.model.chars().any(char::is_control)
        || model
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        || model
            .top_p
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        || model.max_output_tokens == Some(0)
        || model.stop.len() > 128
        || model.stop.iter().any(|stop| stop.len() > 4096)
        || manifest
            .dependencies
            .iter()
            .any(|dependency| !valid_dependency(dependency))
    {
        return Err(AgentDriverError::InvalidManifest);
    }
    Ok(())
}

fn valid_dependency(value: &str) -> bool {
    let Some((kind, remainder)) = value.split_once(':') else {
        return false;
    };
    if kind.is_empty()
        || !kind.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return false;
    }
    let Some((scope, name_and_tag)) = remainder.split_once('/') else {
        return false;
    };
    let (name, tag) = match name_and_tag.split_once(':') {
        Some((name, tag)) => (name, Some(tag)),
        None => (name_and_tag, None),
    };
    valid_part(scope) && valid_part(name) && tag.is_none_or(valid_part)
}

fn valid_part(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}
