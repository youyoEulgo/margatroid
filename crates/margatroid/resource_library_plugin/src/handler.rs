use core_plugin::{Entity, World};
use resource_id_plugin::{ResourceIdLookupError, ResourceRef, WorldResourceIdExt};

use crate::{
    HookResource, PromptResource, ResourceDefinition, ResourceLibraryError, ResourceOwners,
    SandboxResource, SkillResource, ToolResource,
};

fn matches_definition(world: &World, entity: Entity, definition: &ResourceDefinition) -> bool {
    match definition {
        ResourceDefinition::Tool(_) => world.get_component::<ToolResource>(entity).is_some(),
        ResourceDefinition::Skill(_) => world.get_component::<SkillResource>(entity).is_some(),
        ResourceDefinition::Prompt(_) => world.get_component::<PromptResource>(entity).is_some(),
        ResourceDefinition::Sandbox(_) => world.get_component::<SandboxResource>(entity).is_some(),
        ResourceDefinition::Hook(_) => world.get_component::<HookResource>(entity).is_some(),
    }
}

pub fn install_resource(
    world: &mut World,
    reference: ResourceRef,
    owner: Entity,
    definition: ResourceDefinition,
) -> Result<Entity, ResourceLibraryError> {
    if !world.is_alive(owner) {
        return Err(ResourceLibraryError::OwnerMissing(owner));
    }
    if reference.resource_type() != definition.resource_type() {
        return Err(ResourceLibraryError::TypeMismatch);
    }
    let entity = match world.entity_by_resource_id(&reference) {
        Ok(entity) => {
            if !matches_definition(world, entity, &definition)
                || world.get_component::<ResourceOwners>(entity).is_none()
            {
                return Err(ResourceLibraryError::InvalidHost);
            }
            entity
        }
        Err(ResourceIdLookupError::Missing { .. }) => {
            let entity = world.spawn();
            if let Err(error) = world.attach_resource_id(entity, reference) {
                world.despawn(entity);
                return Err(ResourceLibraryError::Attach(error));
            }
            world.insert_component(entity, ResourceOwners::default());
            entity
        }
        Err(error) => return Err(ResourceLibraryError::Lookup(error)),
    };
    match definition {
        ResourceDefinition::Tool(value) => {
            world.insert_component(entity, value);
        }
        ResourceDefinition::Skill(value) => {
            world.insert_component(entity, value);
        }
        ResourceDefinition::Prompt(value) => {
            world.insert_component(entity, value);
        }
        ResourceDefinition::Sandbox(value) => {
            world.insert_component(entity, value);
        }
        ResourceDefinition::Hook(value) => {
            world.insert_component(entity, value);
        }
    }
    world
        .get_component_mut::<ResourceOwners>(entity)
        .expect("resource ownership was installed")
        .acquire(owner);
    Ok(entity)
}

pub fn release_resource_owner(world: &mut World, owner: Entity) {
    let resources = world.query_with::<ResourceOwners>().result();
    for entity in resources {
        let empty = if let Some(owners) = world.get_component_mut::<ResourceOwners>(entity) {
            owners.release(owner);
            owners.is_empty()
        } else {
            false
        };
        if empty {
            world.despawn(entity);
        }
    }
}
