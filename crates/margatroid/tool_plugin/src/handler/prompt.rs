use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    ResourceMapEntry, ToolError, ToolErrorKind, ToolRegisterRequest, ToolRegisterResponse,
};
use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use margatroid_types::ResourceRef;
use serde::Deserialize;

const PROMPT_TYPE: &str = "prompt";
const PROMPT_FILE: &str = "content.md";
const PROMPT_METADATA_FILE: &str = "prompt.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptMetadata {
    role: String,
}

pub(crate) fn prompt_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .filter(|request| request.resource_id.resource_type() == PROMPT_TYPE)
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let result = register_prompt_resource(world, &request);
        world.send_event(ToolRegisterResponse {
            id: request.id,
            agent: request.agent,
            resource_id: request.resource_id,
            alias: request.alias,
            result,
        });
    }
}

fn register_prompt_resource(
    world: &World,
    request: &ToolRegisterRequest,
) -> Result<ResourceMapEntry, ToolError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(ToolError::new(
            ToolErrorKind::InvalidRequest,
            "prompt registration request is invalid",
        ));
    }
    let package = find_prompt_package(super::library_root(world)?, &request.resource_id)?;
    let metadata = read_prompt_metadata(&package)?;
    let content = read_prompt_content(&package)?;
    Ok(ResourceMapEntry {
        resource_id: request.resource_id.clone(),
        resource_name: request
            .alias
            .clone()
            .unwrap_or_else(|| request.resource_id.to_string()),
        alias: request.alias.clone(),
        executor: None,
        template: None,
        content: Some(crate::ResourceContent::Prompt {
            role: metadata.role,
            content: Arc::from(content),
        }),
    })
}

fn read_prompt_metadata(package: &Path) -> Result<PromptMetadata, ToolError> {
    let source = fs::read_to_string(package.join(PROMPT_METADATA_FILE)).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "prompt.toml could not be read",
        )
    })?;
    let metadata: PromptMetadata = toml::from_str(&source)
        .map_err(|_| ToolError::new(ToolErrorKind::InvalidDefinition, "prompt.toml is invalid"))?;
    if !matches!(metadata.role.as_str(), "system" | "user" | "assistant") {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "prompt role must be system, user or assistant",
        ));
    }
    Ok(metadata)
}

fn read_prompt_content(package: &Path) -> Result<String, ToolError> {
    let content = fs::read_to_string(package.join(PROMPT_FILE)).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "content.md could not be read",
        )
    })?;
    if content.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "prompt content must not be empty",
        ));
    }
    Ok(content)
}

pub(super) fn find_prompt_package(
    library_root: &Path,
    resource_id: &ResourceRef,
) -> Result<PathBuf, ToolError> {
    if resource_id.resource_type() != PROMPT_TYPE || resource_id.tag() != "latest" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "prompt resource must use type prompt with tag latest",
        ));
    }
    let package = library_root
        .join("prompts")
        .join(resource_id.scope())
        .join(resource_id.name())
        .join(resource_id.tag());
    if !package.is_dir() {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "prompt package is missing",
        ));
    }
    Ok(package)
}
