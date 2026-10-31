mod components;
mod error;
mod events;
pub mod types;

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use core_plugin::{Entity, World};

pub use components::Prompts;
pub use error::{PromptResourceError, PromptResourceErrorKind};
pub use events::{ImportPrompt, ImportPromptReply};
pub use types::{PromptInput, PromptReadLimits, PromptRole};

use PromptResourceErrorKind as Kind;

pub trait WorldPromptResourceExt {
    fn prompt_path(&self, owner: Entity, alias: &str) -> Result<&Path, PromptResourceError>;
}

impl WorldPromptResourceExt for World {
    fn prompt_path(&self, owner: Entity, alias: &str) -> Result<&Path, PromptResourceError> {
        if !self.is_alive(owner) {
            return Err(PromptResourceError::new(
                Kind::OwnerMissing,
                "owner entity is no longer alive",
            ));
        }
        validate_alias(alias)?;
        self.get_component::<Prompts>(owner)
            .and_then(|entries| entries.get(alias))
            .ok_or_else(|| {
                PromptResourceError::new(Kind::AliasMissing, format!("unregistered alias {alias}"))
            })
    }
}

fn validate_alias(alias: &str) -> Result<(), PromptResourceError> {
    if alias.trim().is_empty()
        || matches!(alias, "." | "..")
        || alias
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(PromptResourceError::new(
            Kind::InvalidAlias,
            "invalid alias",
        ));
    }
    Ok(())
}

fn io_error(path: &Path, error: io::Error) -> PromptResourceError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        Kind::FileMissing
    } else {
        Kind::ReadFailed
    };
    PromptResourceError::new(kind, error.to_string()).with_path(path)
}

fn canonical_file(path: &Path) -> Result<PathBuf, PromptResourceError> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error(path, error))?;
    if !fs::metadata(&canonical)
        .map_err(|error| io_error(path, error))?
        .is_file()
    {
        return Err(
            PromptResourceError::new(Kind::InvalidPath, "expected a regular file").with_path(path),
        );
    }
    Ok(canonical)
}

fn read_text(path: &Path, maximum: usize) -> Result<String, PromptResourceError> {
    let canonical = canonical_file(path)?;
    let mut file = File::open(&canonical).map_err(|error| io_error(path, error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, error))?;
    if !metadata.is_file() {
        return Err(
            PromptResourceError::new(Kind::InvalidPath, "expected a regular file").with_path(path),
        );
    }
    if metadata.len() > maximum as u64 {
        return Err(
            PromptResourceError::new(Kind::TooLarge, "file exceeds byte limit").with_path(path),
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
                PromptResourceError::new(Kind::TooLarge, "file exceeds byte limit").with_path(path),
            );
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).map_err(|_| {
        PromptResourceError::new(Kind::InvalidUtf8, "file is not UTF-8").with_path(path)
    })
}

fn sibling(entry: &Path, name: &str) -> Result<PathBuf, PromptResourceError> {
    let directory = entry.parent().ok_or_else(|| {
        PromptResourceError::new(Kind::InvalidPath, "entry has no parent").with_path(entry)
    })?;
    let candidate = canonical_file(&directory.join(name))?;
    if !candidate.starts_with(directory) {
        return Err(PromptResourceError::new(
            Kind::InvalidPath,
            "resource file escapes entry directory",
        )
        .with_path(candidate));
    }
    Ok(candidate)
}

pub fn read_prompt(
    path: &Path,
    limits: &PromptReadLimits,
) -> Result<PromptInput, PromptResourceError> {
    let origin = canonical_file(path)?;
    let manifest = read_text(&origin, limits.manifest_bytes)?;
    let definition: toml::Value = toml::from_str(&manifest).map_err(|_| {
        PromptResourceError::new(Kind::InvalidDefinition, "invalid prompt TOML manifest")
            .with_path(&origin)
    })?;
    let role = match definition.get("role").and_then(toml::Value::as_str) {
        Some("system") => PromptRole::System,
        Some("user") => PromptRole::User,
        Some("assistant") => PromptRole::Assistant,
        _ => {
            return Err(PromptResourceError::new(
                Kind::InvalidDefinition,
                "role must be system, user or assistant",
            )
            .with_path(&origin))
        }
    };
    let content_path = sibling(&origin, "content.md")?;
    let content = read_text(&content_path, limits.content_bytes)?;
    if content.trim().is_empty() {
        return Err(PromptResourceError::new(
            Kind::InvalidDefinition,
            "prompt content must be nonempty",
        )
        .with_path(content_path));
    }
    Ok(PromptInput { role, content })
}
