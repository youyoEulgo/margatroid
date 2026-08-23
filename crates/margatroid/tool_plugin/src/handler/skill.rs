use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    candidate_resource_entry, ToolCallRequest, ToolError, ToolErrorKind, ToolRegisterRequest,
    ToolRegisterResponse, ToolTemplate,
};
use agent_plugin::Agent;
use app_runtime_plugin::WorldEventExt;
use core_plugin::{Resource, World};
use margatroid_types::ResourceId;
use serde::Deserialize;
use serde_json::json;

const PROVIDER_ID: &str = "skill";
const SKILL_FILE: &str = "SKILL.md";
const SKILL_LOADER_ID: &str = "tool:builtin/skill-loader:latest";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillMetadata {
    name: String,
    description: String,
}

struct SkillDocument {
    metadata: SkillMetadata,
    body: String,
}
pub(crate) struct SkillRoots {
    pub(crate) home_root: Arc<PathBuf>,
}
impl Resource for SkillRoots {}

pub(crate) fn skill_register_system(world: &mut World) {
    let requests = world
        .event_reader::<ToolRegisterRequest>()
        .into_iter()
        .cloned()
        .filter(|request| request.resource_id.resource_type() == "skill")
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
                validate_skill_resource(&request.resource_id)?;
                let roots = world
                    .get_resource::<SkillRoots>()
                    .expect("SkillPlugin is installed");
                let path = find_skill_file(
                    &agent.info.project_root,
                    &agent.info.image_root,
                    &roots.home_root,
                    &request.resource_id,
                )?;
                let document = read_skill_document(&path)?;
                ToolTemplate::new(
                    request.resource_id.to_string(),
                    document.metadata.description,
                    json!({"type":"object"}),
                )
            });
        let result = result.and_then(|template| {
            candidate_resource_entry(
                request.resource_id.clone(),
                request.alias.clone(),
                ResourceId::parse(SKILL_LOADER_ID).expect("built-in Skill loader ID is valid"),
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

pub(crate) fn execute_skill_call(
    world: &World,
    request: &ToolCallRequest,
) -> Result<String, ToolError> {
    let agent = world.get_component::<Agent>(request.agent).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::ToolEnvironmentMissing,
            "agent tool environment is missing",
        )
    })?;
    validate_skill_resource(&request.resource_id)?;
    let roots = world
        .get_resource::<SkillRoots>()
        .expect("SkillPlugin is installed");
    let path = find_skill_file(
        &agent.info.project_root,
        &agent.info.image_root,
        &roots.home_root,
        &request.resource_id,
    )?;
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&request.arguments)
        .map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "skill arguments must be a JSON object",
            )
        })?;
    read_skill_document(&path).map(|document| document.body)
}

fn read_skill_document(path: &Path) -> Result<SkillDocument, ToolError> {
    let source = fs::read_to_string(path).map_err(|_| {
        ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "skill file could not be read",
        )
    })?;
    let remainder = source.strip_prefix("+++\n").ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill file must begin with TOML frontmatter",
        )
    })?;
    let (metadata_source, body) = remainder.split_once("\n+++\n").ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill TOML frontmatter is not terminated",
        )
    })?;
    let metadata = toml::from_str::<SkillMetadata>(metadata_source).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill TOML frontmatter is invalid",
        )
    })?;
    if metadata.name.trim().is_empty() || metadata.description.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill name and description must not be empty",
        ));
    }
    let body = body.trim().to_owned();
    if body.is_empty() {
        return Err(ToolError::new(
            ToolErrorKind::InvalidDefinition,
            "skill body must not be empty",
        ));
    }
    Ok(SkillDocument { metadata, body })
}

fn find_skill_file(
    project_root: &Path,
    image_root: &Path,
    home_root: &Path,
    resource: &ResourceId,
) -> Result<PathBuf, ToolError> {
    let candidates = [
        project_root.join(".margatroid").join("skills"),
        image_root.join("skills"),
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
                    "skill package could not be inspected",
                ));
            }
        };
        if !metadata.is_dir() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "skill package is not a directory",
            ));
        }
        let path = package.join(SKILL_FILE);
        if !path.is_file() {
            return Err(ToolError::new(
                ToolErrorKind::ResourceResolutionFailed,
                "skill package does not contain SKILL.md",
            ));
        }
        return Ok(path);
    }
    Err(ToolError::new(
        ToolErrorKind::ResourceResolutionFailed,
        "skill file was not found",
    ))
}

fn validate_skill_resource(resource: &ResourceId) -> Result<(), ToolError> {
    if resource.resource_type() != PROVIDER_ID || resource.tag() != "latest" {
        return Err(ToolError::new(
            ToolErrorKind::ResourceResolutionFailed,
            "skill resource must use type skill and the supported latest tag",
        ));
    }
    Ok(())
}
