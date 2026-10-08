mod error;
mod events;
mod handler;
mod system;
mod types;

use core_plugin::{Entity, Plugin, Resource, World};

pub use error::{ResourceIdAttachError, ResourceIdError, ResourceIdLookupError};
pub use types::{ResourceId, ResourceRef};

pub struct ResourceIdPlugin;

pub struct ResourceIdPluginInstalled;
impl Resource for ResourceIdPluginInstalled {}

impl Plugin for ResourceIdPlugin {
    fn build(self, app: &mut core_plugin::App) {
        if app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("ResourceIdPlugin is already installed");
        }
        app.world_mut().insert_resource(ResourceIdPluginInstalled);
    }
}

pub fn attach_resource_id(
    world: &mut World,
    entity: Entity,
    id: ResourceRef,
) -> Result<(), ResourceIdAttachError> {
    if !world.is_alive(entity) {
        return Err(ResourceIdAttachError::MissingEntity { entity });
    }
    match world.entity_by_resource_id(&id) {
        Err(ResourceIdLookupError::Missing { .. }) => {}
        Ok(owner) if owner == entity => return Ok(()),
        Ok(owner) => {
            return Err(ResourceIdAttachError::Lookup(
                ResourceIdLookupError::Duplicate {
                    id,
                    entities: vec![owner, entity],
                },
            ))
        }
        Err(error) => return Err(ResourceIdAttachError::Lookup(error)),
    }
    world.insert_component(entity, ResourceId(id));
    Ok(())
}

pub trait WorldResourceIdExt {
    fn attach_resource_id(
        &mut self,
        entity: Entity,
        id: ResourceRef,
    ) -> Result<(), ResourceIdAttachError>;
    fn entity_by_resource_id(&self, id: &ResourceRef) -> Result<Entity, ResourceIdLookupError>;
}

impl WorldResourceIdExt for World {
    fn attach_resource_id(
        &mut self,
        entity: Entity,
        id: ResourceRef,
    ) -> Result<(), ResourceIdAttachError> {
        attach_resource_id(self, entity, id)
    }

    fn entity_by_resource_id(&self, id: &ResourceRef) -> Result<Entity, ResourceIdLookupError> {
        if !self.contains_resource::<ResourceIdPluginInstalled>() {
            return Err(ResourceIdLookupError::PluginMissing);
        }
        let mut entities = self
            .query_with::<ResourceId>()
            .result()
            .into_iter()
            .filter(|entity| {
                self.get_component::<ResourceId>(*entity)
                    .is_some_and(|value| value.as_ref() == id)
            })
            .collect::<Vec<_>>();
        entities.sort_by_key(|entity| entity.index());
        match entities.as_slice() {
            [] => Err(ResourceIdLookupError::Missing { id: id.clone() }),
            [entity] => Ok(*entity),
            _ => Err(ResourceIdLookupError::Duplicate {
                id: id.clone(),
                entities,
            }),
        }
    }
}
