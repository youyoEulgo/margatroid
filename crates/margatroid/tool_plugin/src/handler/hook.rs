use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::{
    candidate_resource_entry, ToolCallRequest, ToolCallResponse, ToolError, ToolErrorKind,
    ToolRegisterRequest, ToolRegisterResponse, ToolTemplate,
};
use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use core_plugin::{Resource, World};
use margatroid_types::ResourceId;
use serde::Deserialize;

const PROVIDER_ID: &str = "hook";
const HOOK_FILE: &str = "hook.toml";
const HOOK_SCHEMA_FILE: &str = "input.schema.json";
const HOOK_EXECUTOR_ID: &str = "tool:builtin/hook:latest";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HookMetadata {
    name: String,
    description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookErrorKind {
    InvalidRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookError {
    kind: HookErrorKind,
    message: String,
}

impl HookError {
    fn new(kind: HookErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> HookErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for HookError {}

pub(crate) struct HookRoots {
    pub(crate) home_root: Arc<PathBuf>,
}
impl Resource for HookRoots {}

pub(crate) fn hook_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .filter(|request| {
            request.resource_id.resource_type() == "hook"
                || request.resource_id.to_string() == HOOK_EXECUTOR_ID
        })
        .collect::<Vec<_>>();
    for request in requests {
        let result = world
            .get_component::<Agent>(request.agent)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::ToolEnvironmentMissing,
                    "agent tool environment is missing",
                )
            })
            .and_then(|agent| {
                validate_hook_resource(&request.resource_id)?;
                let roots = world
                    .get_resource::<HookRoots>()
                    .expect("HookPlugin is installed");
                let package_root = find_hook_package(
                    &agent.info.project_root,
                    &agent.info.image_root,
                    &roots.home_root,
                    &request.resource_id,
                )?;
                let metadata = read_hook_metadata(&package_root)?;
                let schema = read_hook_schema(&package_root)?;
                ToolTemplate::new(
                    request.resource_id.to_string(),
                    metadata.description,
                    schema,
                )
            });
        let result = result.and_then(|template| {
            candidate_resource_entry(
                request.resource_id.clone(),
                request.alias.clone(),
                ResourceId::parse(HOOK_EXECUTOR_ID)
                    .expect("built-in Hook executor ID must be valid"),
                template,
            )
        });
        world.send_event(ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

pub(crate) fn hook_tool_call_system(world: &mut World) {
    let hook_tool_id =
        ResourceId::parse(HOOK_EXECUTOR_ID).expect("built-in Hook tool ID must be valid");
    let calls = world
        .event_reader::<ToolCallRequest>()
        .into_iter()
        .cloned()
        .filter(|call| call.tool_id == hook_tool_id)
        .collect::<Vec<_>>();
    for call in calls {
        world.send_event(ToolCallResponse {
            turn_id: call.turn_id,
            agent: call.agent,
            tool_call_id: call.tool_call_id,
            result: Ok(String::new()),
        });
    }
}

fn read_hook_metadata(package_root: &Path) -> Result<HookMetadata, ToolError> {
    let source = fs::read_to_string(package_root.join(HOOK_FILE)).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "hook.toml could not be read",
        )
    })?;
    let metadata: HookMetadata = toml::from_str(&source)
        .map_err(|_| ToolError::new(ToolErrorKind::InvalidDefinition, "hook.toml is invalid"))?;
    if metadata.name.trim().is_empty() || metadata.description.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "hook name and description must not be empty",
        ));
    }
    Ok(metadata)
}

fn read_hook_schema(package_root: &Path) -> Result<serde_json::Value, ToolError> {
    let source = fs::read_to_string(package_root.join(HOOK_SCHEMA_FILE)).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "input.schema.json could not be read",
        )
    })?;
    serde_json::from_str(&source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "input.schema.json is invalid",
        )
    })
}

fn find_hook_package(
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
    resource: &ResourceId,
) -> Result<PathBuf, ToolError> {
    let candidates = [
        project_root.join(".margatroid").join("hooks"),
        image_root.join("hooks"),
        home_root.to_path_buf(),
    ];
    for root in candidates {
        let package = root
            .join(resource.scope())
            .join(resource.name())
            .join(resource.tag());
        let metadata = match fs::metadata(&package) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ToolError::new(
                    ToolErrorKind::ResourceResolutionFailed,
                    "hook package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "hook package is not a directory",
            ));
        }
        if !package.join(HOOK_FILE).is_file() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "hook package does not contain hook.toml",
            ));
        }
        if !package.join(HOOK_SCHEMA_FILE).is_file() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "hook package does not contain input.schema.json",
            ));
        }
        return Ok(package);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "hook resource was not found",
    ))
}

fn validate_hook_resource(resource: &ResourceId) -> Result<(), ToolError> {
    if resource.resource_type() != PROVIDER_ID || resource.tag() != "latest" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "hook resource must use type hook and the supported latest tag",
        ));
    }
    Ok(())
}

fn normalize_root(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            normalized.push(component.as_os_str());
        }
    }
    Some(normalized)
}
