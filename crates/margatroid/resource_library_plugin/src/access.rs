use std::path::Path;

use core_plugin::{Entity, World};
use resource_id_plugin::{ResourceRef, WorldResourceIdExt};

use crate::{
    HookResource, PromptResource, ResourceLibraryError, SandboxResource, SkillResource,
    ToolResource,
};

pub fn find_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<Entity, ResourceLibraryError> {
    world
        .entity_by_resource_id(reference)
        .map_err(ResourceLibraryError::Lookup)
}

pub fn tool_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<ToolResource, ResourceLibraryError> {
    let entity = find_resource(world, reference)?;
    world
        .get_component::<ToolResource>(entity)
        .cloned()
        .ok_or_else(|| ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "tool",
        })
}

pub fn skill_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<SkillResource, ResourceLibraryError> {
    let entity = find_resource(world, reference)?;
    world
        .get_component::<SkillResource>(entity)
        .cloned()
        .ok_or_else(|| ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "skill",
        })
}

pub fn prompt_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<PromptResource, ResourceLibraryError> {
    let entity = find_resource(world, reference)?;
    world
        .get_component::<PromptResource>(entity)
        .cloned()
        .ok_or_else(|| ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "prompt",
        })
}

pub fn sandbox_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<SandboxResource, ResourceLibraryError> {
    let entity = find_resource(world, reference)?;
    world
        .get_component::<SandboxResource>(entity)
        .cloned()
        .ok_or_else(|| ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "sandbox",
        })
}

pub fn hook_resource(
    world: &World,
    reference: &ResourceRef,
) -> Result<HookResource, ResourceLibraryError> {
    let entity = find_resource(world, reference)?;
    world
        .get_component::<HookResource>(entity)
        .cloned()
        .ok_or_else(|| ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "hook",
        })
}

pub fn read_tool_script(
    world: &World,
    reference: &ResourceRef,
) -> Result<String, ResourceLibraryError> {
    let tool = tool_resource(world, reference)?;
    read_text(reference, &tool.script_path)
}

pub fn read_skill_content(
    world: &World,
    reference: &ResourceRef,
) -> Result<String, ResourceLibraryError> {
    let skill = skill_resource(world, reference)?;
    read_text(reference, &skill.content_path)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecutableDefinition {
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub fn tool_definition(
    world: &World,
    reference: &ResourceRef,
) -> Result<ExecutableDefinition, ResourceLibraryError> {
    match reference.resource_type() {
        "tool" => {
            let tool = tool_resource(world, reference)?;
            Ok(ExecutableDefinition {
                description: tool.description,
                input_schema: tool.input_schema,
            })
        }
        "skill" => {
            let skill = skill_resource(world, reference)?;
            Ok(ExecutableDefinition {
                description: skill.description,
                input_schema: serde_json::json!({ "type": "object" }),
            })
        }
        "hook" => {
            let hook = hook_resource(world, reference)?;
            Ok(ExecutableDefinition {
                description: hook.description,
                input_schema: hook.input_schema,
            })
        }
        _ => Err(ResourceLibraryError::WrongType {
            reference: reference.clone(),
            expected: "executable resource",
        }),
    }
}

fn read_text(reference: &ResourceRef, path: &Path) -> Result<String, ResourceLibraryError> {
    std::fs::read_to_string(path).map_err(|error| ResourceLibraryError::ContentUnreadable {
        reference: reference.clone(),
        reason: error.to_string(),
    })
}
