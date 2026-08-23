mod error;
mod events;
mod handler;
mod system;
mod types;

use core_plugin::{Entity, Plugin, Resource, World};

pub use error::{ResourceIdError, ResourceIdLookupError};
pub use types::ResourceId;

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

pub trait WorldResourceIdExt {
    fn entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError>;
}

impl WorldResourceIdExt for World {
    fn entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError> {
        if !self.contains_resource::<ResourceIdPluginInstalled>() {
            return Err(ResourceIdLookupError::PluginMissing);
        }
        let mut entities = self
            .query_with::<ResourceId>()
            .result()
            .into_iter()
            .filter(|entity| {
                self.get_component::<ResourceId>(*entity)
                    .is_some_and(|value| value == id)
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
