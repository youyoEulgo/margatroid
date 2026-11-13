mod components;
mod error;
mod events;
pub mod types;

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use core_plugin::{Entity, World};

pub use components::{Sandboxes, SelectedSandbox};
pub use error::{SandboxResourceError, SandboxResourceErrorKind};
pub use events::{ImportSandbox, ImportSandboxReply};
pub use types::{SandboxInput, SandboxReadLimits};

use SandboxResourceErrorKind as Kind;

pub trait WorldSandboxResourceExt {
    fn sandbox_path(&self, owner: Entity, alias: &str) -> Result<&Path, SandboxResourceError>;
    fn sandbox_select(
        &mut self,
        owner: Entity,
        alias: Option<String>,
    ) -> Result<(), SandboxResourceError>;
}

impl WorldSandboxResourceExt for World {
    fn sandbox_path(&self, owner: Entity, alias: &str) -> Result<&Path, SandboxResourceError> {
        if !self.is_alive(owner) {
            return Err(SandboxResourceError::new(
                Kind::OwnerMissing,
                "owner entity is no longer alive",
            ));
        }
        validate_alias(alias)?;
        self.get_component::<Sandboxes>(owner)
            .and_then(|entries| entries.get(alias))
            .ok_or_else(|| {
                SandboxResourceError::new(Kind::AliasMissing, format!("unregistered alias {alias}"))
            })
    }
    fn sandbox_select(
        &mut self,
        owner: Entity,
        alias: Option<String>,
    ) -> Result<(), SandboxResourceError> {
        if !self.is_alive(owner) {
            return Err(SandboxResourceError::new(
                Kind::OwnerMissing,
                "owner entity is no longer alive",
            ));
        }
        if let Some(alias) = alias.as_deref() {
            self.sandbox_path(owner, alias)?;
        }
        self.get_component_mut::<SelectedSandbox>(owner)
            .ok_or_else(|| {
                SandboxResourceError::new(
                    SandboxResourceErrorKind::OwnerMissing,
                    "sandbox selection context missing",
                )
            })?
            .set(alias);
        Ok(())
    }
}

fn validate_alias(alias: &str) -> Result<(), SandboxResourceError> {
    if alias.trim().is_empty()
        || matches!(alias, "." | "..")
        || alias
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(SandboxResourceError::new(
            Kind::InvalidAlias,
            "invalid alias",
        ));
    }
    Ok(())
}

fn io_error(path: &Path, error: io::Error) -> SandboxResourceError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        Kind::FileMissing
    } else {
        Kind::ReadFailed
    };
    SandboxResourceError::new(kind, error.to_string()).with_path(path)
}

fn canonical_file(path: &Path) -> Result<PathBuf, SandboxResourceError> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error(path, error))?;
    if !fs::metadata(&canonical)
        .map_err(|error| io_error(path, error))?
        .is_file()
    {
        return Err(
            SandboxResourceError::new(Kind::InvalidPath, "expected a regular file").with_path(path),
        );
    }
    Ok(canonical)
}

fn read_text(path: &Path, maximum: usize) -> Result<String, SandboxResourceError> {
    let canonical = canonical_file(path)?;
    let mut file = File::open(&canonical).map_err(|error| io_error(path, error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, error))?;
    if !metadata.is_file() {
        return Err(
            SandboxResourceError::new(Kind::InvalidPath, "expected a regular file").with_path(path),
        );
    }
    if metadata.len() > maximum as u64 {
        return Err(
            SandboxResourceError::new(Kind::TooLarge, "file exceeds byte limit").with_path(path),
        );
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        if count > maximum.saturating_sub(bytes.len()) {
            return Err(
                SandboxResourceError::new(Kind::TooLarge, "file exceeds byte limit")
                    .with_path(path),
            );
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).map_err(|_| {
        SandboxResourceError::new(Kind::InvalidUtf8, "file is not UTF-8").with_path(path)
    })
}

pub fn read_sandbox(
    path: &Path,
    limits: &SandboxReadLimits,
) -> Result<SandboxInput, SandboxResourceError> {
    let origin = canonical_file(path)?;
    let source = read_text(&origin, limits.policy_bytes)?;
    if source.trim().is_empty() {
        return Err(SandboxResourceError::new(
            Kind::InvalidDefinition,
            "sandbox policy must be nonempty",
        )
        .with_path(&origin));
    }
    let policy: serde_json::Value = serde_json::from_str(&source).map_err(|error| {
        SandboxResourceError::new(Kind::InvalidDefinition, error.to_string()).with_path(&origin)
    })?;
    if !policy.is_object() {
        return Err(SandboxResourceError::new(
            Kind::InvalidDefinition,
            "sandbox policy must be a JSON object",
        )
        .with_path(&origin));
    }
    Ok(SandboxInput { policy, origin })
}

pub type SandboxSyncBinding = client_core::ProbeFactory<Vec<(String, std::path::PathBuf)>>;
pub type SandboxSelectionSyncBinding = client_core::ProbeFactory<Option<String>>;
