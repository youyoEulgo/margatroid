use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    ResourceContent, ResourceMapEntry, ToolError, ToolErrorKind, ToolRegisterRequest,
    ToolRegisterResponse,
};
use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use core_plugin::{Resource, World};
use margatroid_types::ResourceId;
use serde::Deserialize;

pub(crate) use crate::handler::pty::SHELL_SCRIPT_FILE;

const SHELL_TYPE: &str = "shell";
const SHELL_FILE: &str = "shell.toml";
const SHELL_SCHEMA_FILE: &str = "input.schema.json";
const SHELL_COMMAND_PROPERTY: &str = "cmd";

/// How much of a shell package may be read. A shell is only an interpreter
/// script now, so the limits that governed running a command through it are
/// gone with that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellExecutionLimits {
    max_definition_bytes: usize,
    max_script_bytes: usize,
}

impl ShellExecutionLimits {
    pub fn new(max_definition_bytes: usize, max_script_bytes: usize) -> Result<Self, ShellError> {
        if max_definition_bytes == 0 || max_script_bytes == 0 {
            return Err(ShellError::new(
                ShellErrorKind::InvalidLimits,
                "Shell limits must be nonzero",
            ));
        }
        Ok(Self {
            max_definition_bytes,
            max_script_bytes,
        })
    }
}

impl Default for ShellExecutionLimits {
    fn default() -> Self {
        Self::new(64 * 1024, 4 * 1024 * 1024).expect("default Shell limits are valid")
    }
}

impl Resource for ShellExecutionLimits {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellErrorKind {
    InvalidLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellError {
    kind: ShellErrorKind,
    message: String,
}

impl ShellError {
    fn new(kind: ShellErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ShellError {}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellMetadata {
    schema_version: u32,
    name: String,
    description: String,
}

pub(crate) fn shell_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .filter(|request| request.resource_id.resource_type() == "shell")
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_shell_resource(world, &request);
        world.send_event(ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn register_shell_resource(
    world: &mut World,
    request: &ToolRegisterRequest,
) -> Result<ResourceMapEntry, ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "Shell resource registration request is invalid",
        ));
    }
    validate_shell_resource(&request.resource_id)?;
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    let limits = world
        .get_resource::<ShellExecutionLimits>()
        .expect("ShellPlugin is installed");
    let package_root = find_shell_package(
        &agent.info.project_root,
        &agent.info.image_root,
        &request.resource_id,
    )?;
    let metadata = read_bounded_sync(
        &package_root.join(SHELL_FILE),
        limits.max_definition_bytes,
        "Shell metadata",
    )?;
    let schema = read_bounded_sync(
        &package_root.join(SHELL_SCHEMA_FILE),
        limits.max_definition_bytes,
        "Shell schema",
    )?;
    let script = read_bounded_sync(
        &package_root.join(SHELL_SCRIPT_FILE),
        limits.max_script_bytes,
        "Shell script",
    )?;
    if script.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell script is empty",
        ));
    }
    let _definition = parse_shell_definition(&metadata, &schema, &request.resource_id)?;
    // A shell is not a tool the model calls, so the entry carries no executor and
    // no template: it exists to hand its interpreter to whatever runs commands.
    // Importing it is what puts the script on the agent, which is how a Lua tool
    // reaches a terminal. A template here would also make the import look like a
    // visible tool and keep the script out of the agent's shells.
    Ok(ResourceMapEntry {
        resource_id: request.resource_id.clone(),
        resource_name: request
            .alias
            .clone()
            .unwrap_or_else(|| request.resource_id.to_string()),
        alias: request.alias.clone(),
        tool_id: None,
        template: None,
        content: Some(ResourceContent::Shell {
            script: Arc::from(script),
        }),
    })
}

fn find_shell_package(
    project_root: &Path,
    image_root: &Path,
    resource_id: &ResourceId,
) -> Result<PathBuf, ToolError> {
    validate_shell_resource(resource_id)?;
    let roots = [
        project_root.join(".margatroid").join("shells"),
        image_root.join("shells"),
    ];
    for root in roots {
        let package = root
            .join(resource_id.scope())
            .join(resource_id.name())
            .join(resource_id.tag());
        let metadata = match fs::metadata(&package) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "Shell package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "Shell package is not a directory",
            ));
        }
        for file in [SHELL_FILE, SHELL_SCHEMA_FILE, SHELL_SCRIPT_FILE] {
            if !package.join(file).is_file() {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "Shell package is incomplete",
                ));
            }
        }
        return Ok(package);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "Shell resource was not found",
    ))
}

/// Check that a shell package declares what a command runner needs. Nothing is
/// returned: the metadata is validated and dropped, because a shell is not a
/// tool the model calls and nothing reads its name or description afterwards.
fn parse_shell_definition(
    metadata_source: &str,
    schema_source: &str,
    resource_id: &ResourceId,
) -> Result<(), ToolError> {
    let metadata = toml::from_str::<ShellMetadata>(metadata_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell metadata is invalid",
        )
    })?;
    if metadata.schema_version != 1
        || metadata.name.trim().is_empty()
        || metadata.name != resource_id.name()
        || metadata.description.trim().is_empty()
    {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell metadata does not match the resource",
        ));
    }
    let parameters = serde_json::from_str::<serde_json::Value>(schema_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema is invalid JSON",
        )
    })?;
    if !parameters.is_object() || jsonschema::validator_for(&parameters).is_err() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema is invalid",
        ));
    }
    let command_property = parameters
        .get("properties")
        .and_then(|properties| properties.get(SHELL_COMMAND_PROPERTY))
        .and_then(|command| command.get("type"))
        .and_then(serde_json::Value::as_str);
    let requires_command = parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|value| value == SHELL_COMMAND_PROPERTY));
    if command_property != Some("string") || !requires_command {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "Shell input schema must require a string command",
        ));
    }
    Ok(())
}

fn read_bounded_sync(path: &Path, limit: usize, label: &str) -> Result<String, ToolError> {
    let file = fs::File::open(path).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            format!("{label} could not be read"),
        )
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                format!("{label} could not be read"),
            )
        })?;
    decode_bounded(bytes, limit, label)
}

fn decode_bounded(bytes: Vec<u8>, limit: usize, label: &str) -> Result<String, ToolError> {
    if bytes.len() > limit {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("{label} exceeds the size limit"),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            format!("{label} must be UTF-8"),
        )
    })
}

fn validate_shell_resource(resource_id: &ResourceId) -> Result<(), ToolError> {
    if resource_id.resource_type() != SHELL_TYPE {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "Shell resource must use type shell",
        ));
    }
    Ok(())
}
