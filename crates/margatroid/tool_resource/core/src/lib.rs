mod components;
mod error;
mod events;
pub mod types;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use core_plugin::{Entity, World};

pub use components::Tools;
pub use error::{ToolResourceError, ToolResourceErrorKind};
pub use events::{ImportTool, ImportToolReply};
pub use types::{InjectedToolDefinition, ToolEntry, ToolInput};

use ToolResourceErrorKind as Kind;

pub trait WorldToolResourceExt {
    fn tool_entry(&self, owner: Entity, alias: &str) -> Result<&ToolEntry, ToolResourceError>;
    fn tool_path(&self, owner: Entity, alias: &str) -> Result<&Path, ToolResourceError>;
}

impl WorldToolResourceExt for World {
    fn tool_path(&self, owner: Entity, alias: &str) -> Result<&Path, ToolResourceError> {
        Ok(self.tool_entry(owner, alias)?.path.as_path())
    }

    fn tool_entry(&self, owner: Entity, alias: &str) -> Result<&ToolEntry, ToolResourceError> {
        if !self.is_alive(owner) {
            return Err(ToolResourceError::new(
                Kind::OwnerMissing,
                "owner entity is no longer alive",
            ));
        }
        validate_alias(alias)?;
        self.get_component::<Tools>(owner)
            .and_then(|entries| entries.get(alias))
            .ok_or_else(|| {
                ToolResourceError::new(Kind::AliasMissing, format!("unregistered alias {alias}"))
            })
    }
}

fn validate_alias(alias: &str) -> Result<(), ToolResourceError> {
    let mut bytes = alias.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ToolResourceError::new(Kind::InvalidAlias, "invalid alias"));
    }
    Ok(())
}

fn io_error(path: &Path, error: io::Error) -> ToolResourceError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        Kind::FileMissing
    } else {
        Kind::ReadFailed
    };
    ToolResourceError::new(kind, error.to_string()).with_path(path)
}

fn canonical_file(path: &Path) -> Result<PathBuf, ToolResourceError> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error(path, error))?;
    if !fs::metadata(&canonical)
        .map_err(|error| io_error(path, error))?
        .is_file()
    {
        return Err(
            ToolResourceError::new(Kind::InvalidPath, "expected a regular file").with_path(path),
        );
    }
    Ok(canonical)
}

fn read_text(path: &Path) -> Result<String, ToolResourceError> {
    let canonical = canonical_file(path)?;
    let bytes = fs::read(&canonical).map_err(|error| io_error(path, error))?;
    String::from_utf8(bytes)
        .map_err(|_| ToolResourceError::new(Kind::InvalidUtf8, "file is not UTF-8").with_path(path))
}

fn sibling(entry: &Path, name: &str) -> Result<PathBuf, ToolResourceError> {
    let directory = entry.parent().ok_or_else(|| {
        ToolResourceError::new(Kind::InvalidPath, "entry has no parent").with_path(entry)
    })?;
    let candidate = canonical_file(&directory.join(name))?;
    if !candidate.starts_with(directory) {
        return Err(ToolResourceError::new(
            Kind::InvalidPath,
            "resource file escapes entry directory",
        )
        .with_path(candidate));
    }
    Ok(candidate)
}

pub fn read_tool(path: &Path) -> Result<ToolInput, ToolResourceError> {
    let origin = canonical_file(path)?;
    let manifest = read_text(&origin)?;
    let definition: toml::Value = toml::from_str(&manifest).map_err(|_| {
        ToolResourceError::new(Kind::InvalidDefinition, "invalid tool TOML manifest")
            .with_path(&origin)
    })?;
    if definition
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        != Some(1)
    {
        return Err(
            ToolResourceError::new(Kind::InvalidDefinition, "schema_version must be 1")
                .with_path(&origin),
        );
    }
    let description = definition
        .get("description")
        .and_then(toml::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            ToolResourceError::new(Kind::InvalidDefinition, "description must be nonempty")
                .with_path(&origin)
        })?
        .to_owned();
    let schema_path = sibling(&origin, "input.schema.json")?;
    let schema_source = read_text(&schema_path)?;
    let input_schema: serde_json::Value =
        serde_json::from_str(&schema_source).map_err(|error| {
            ToolResourceError::new(Kind::InvalidDefinition, error.to_string())
                .with_path(&schema_path)
        })?;
    validate_schema(&input_schema).map_err(|error| error.with_path(&schema_path))?;
    let program = read_text(&sibling(&origin, "main.lua")?)?;
    Ok(ToolInput {
        description,
        input_schema,
        program,
        origin,
    })
}

fn validate_schema(input_schema: &serde_json::Value) -> Result<(), ToolResourceError> {
    if !input_schema.is_object() {
        return Err(ToolResourceError::new(
            Kind::InvalidDefinition,
            "schema must be a JSON object",
        ));
    }
    if has_external_reference(input_schema) {
        return Err(ToolResourceError::new(
            Kind::InvalidDefinition,
            "external schema references are not allowed",
        ));
    }
    jsonschema::options()
        .with_retriever(NoExternalSchema)
        .build(input_schema)
        .map_err(|_| {
            ToolResourceError::new(
                Kind::InvalidDefinition,
                "invalid schema or external schema reference",
            )
        })?;
    Ok(())
}

fn has_external_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(entries) => entries.iter().any(|(key, value)| {
            (matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef")
                && value
                    .as_str()
                    .is_some_and(|reference| !reference.starts_with('#')))
                || has_external_reference(value)
        }),
        serde_json::Value::Array(entries) => entries.iter().any(has_external_reference),
        _ => false,
    }
}

struct NoExternalSchema;

impl jsonschema::Retrieve for NoExternalSchema {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema references are not allowed".into())
    }
}

pub type ToolResourceSyncBinding = client_core::ProbeFactory<Vec<(String, ToolEntry)>>;
