use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use core_plugin::{Component, Entity, Resource, World};

use crate::{
    LibraryComponent, ResourceError, ResourceID, ResourcePluginInstalled, ResourcesComponent,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceComponentKind {
    Library,
    Resources,
}

#[derive(Clone, Copy, Debug)]
pub struct ResourceTypeRegistration {
    pub component_type: TypeId,
    pub kind: ResourceComponentKind,
    pub query: fn(&World, &ResourceID) -> Vec<Entity>,
    pub contains: fn(&World, Entity) -> bool,
}

#[derive(Default, Debug)]
pub struct ResourceTypeRegistry {
    registrations: HashMap<String, ResourceTypeRegistration>,
}

impl Resource for ResourceTypeRegistry {}

impl ResourceTypeRegistry {
    pub fn get(&self, resource_type: &str) -> Option<&ResourceTypeRegistration> {
        self.registrations.get(resource_type)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ResourceTypeRegistration)> {
        self.registrations
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
    }
}

fn registry(world: &World) -> Result<&ResourceTypeRegistry, ResourceError> {
    if !world.contains_resource::<ResourcePluginInstalled>() {
        return Err(ResourceError::PluginMissing);
    }
    world
        .get_resource::<ResourceTypeRegistry>()
        .ok_or(ResourceError::PluginMissing)
}

fn register<C: Component>(
    world: &mut World,
    resource_type: &str,
    kind: ResourceComponentKind,
    query: fn(&World, &ResourceID) -> Vec<Entity>,
) -> Result<(), ResourceError> {
    let registry = registry(world)?;
    crate::types::validate_type(resource_type).map_err(|_| ResourceError::InvalidResourceType {
        resource_type: resource_type.into(),
    })?;
    let component_type = TypeId::of::<C>();
    if let Some(entry) = registry.get(resource_type) {
        return if entry.component_type == component_type && entry.kind == kind {
            Ok(())
        } else {
            Err(ResourceError::TypeRegistrationConflict {
                resource_type: resource_type.into(),
            })
        };
    }
    if registry
        .registrations
        .values()
        .any(|entry| entry.component_type == component_type)
    {
        return Err(ResourceError::TypeRegistrationConflict {
            resource_type: resource_type.into(),
        });
    }
    world
        .get_resource_mut::<ResourceTypeRegistry>()
        .unwrap()
        .registrations
        .insert(
            resource_type.into(),
            ResourceTypeRegistration {
                component_type,
                kind,
                query,
                contains: has_component::<C>,
            },
        );
    Ok(())
}

pub fn register_library_type<C: LibraryComponent>(
    world: &mut World,
    resource_type: &str,
) -> Result<(), ResourceError> {
    register::<C>(
        world,
        resource_type,
        ResourceComponentKind::Library,
        query_library::<C>,
    )
}

pub fn register_resources_type<C: ResourcesComponent>(
    world: &mut World,
    resource_type: &str,
) -> Result<(), ResourceError> {
    register::<C>(
        world,
        resource_type,
        ResourceComponentKind::Resources,
        query_resources::<C>,
    )
}

fn registered_type<C: Component>(
    world: &World,
    kind: ResourceComponentKind,
) -> Result<&str, ResourceError> {
    let registry = registry(world)?;
    let component_type = TypeId::of::<C>();
    let (name, entry) = registry
        .iter()
        .find(|(_, entry)| entry.component_type == component_type)
        .ok_or(ResourceError::ComponentNotRegistered { component_type })?;
    if entry.kind != kind {
        return Err(ResourceError::ComponentKindMismatch { component_type });
    }
    Ok(name)
}

fn validate_address(id: &ResourceID, expected: &str) -> Result<(), ResourceError> {
    if id.resource_type() != expected {
        return Err(ResourceError::AddressTypeMismatch {
            id: id.clone(),
            expected: expected.into(),
        });
    }
    Ok(())
}

pub fn insert_library_component<C: LibraryComponent>(
    world: &mut World,
    entity: Entity,
    component: C,
) -> Result<(), ResourceError> {
    if !world.is_alive(entity) {
        return Err(ResourceError::EntityNotAlive { entity });
    }
    let expected = registered_type::<C>(world, ResourceComponentKind::Library)?;
    let id = component.resource_id();
    validate_address(id, expected)?;
    if registry(world)?.registrations.values().any(|entry| {
        entry.kind == ResourceComponentKind::Library
            && entry.component_type != TypeId::of::<C>()
            && (entry.contains)(world, entity)
    }) {
        return Err(ResourceError::LibraryConflict { entity });
    }
    let entities: Vec<_> = find_resource_entities(world, id)?
        .into_iter()
        .filter(|owner| *owner != entity)
        .collect();
    if !entities.is_empty() {
        return Err(ResourceError::LibraryAddressOccupied {
            id: id.clone(),
            entities,
        });
    }
    world.insert_component(entity, component);
    Ok(())
}

pub fn insert_resources_component<C: ResourcesComponent>(
    world: &mut World,
    entity: Entity,
    component: C,
) -> Result<(), ResourceError> {
    if !world.is_alive(entity) {
        return Err(ResourceError::EntityNotAlive { entity });
    }
    let expected = registered_type::<C>(world, ResourceComponentKind::Resources)?;
    let mut seen = HashSet::new();
    for id in component.resource_ids() {
        validate_address(id, expected)?;
        if !seen.insert(id) {
            return Err(ResourceError::DuplicateEntryID { id: id.clone() });
        }
    }
    world.insert_component(entity, component);
    Ok(())
}

pub fn find_resource_entities(
    world: &World,
    id: &ResourceID,
) -> Result<Vec<Entity>, ResourceError> {
    let entry = registry(world)?.get(id.resource_type()).ok_or_else(|| {
        ResourceError::TypeNotRegistered {
            resource_type: id.resource_type().into(),
        }
    })?;
    let mut entities = (entry.query)(world, id);
    entities.sort_by_key(|entity| (entity.generation(), entity.index()));
    entities.dedup();
    Ok(entities)
}

fn query_library<C: LibraryComponent>(world: &World, id: &ResourceID) -> Vec<Entity> {
    world
        .query_with::<C>()
        .result()
        .into_iter()
        .filter(|entity| {
            world
                .get_component::<C>(*entity)
                .is_some_and(|component| component.resource_id() == id)
        })
        .collect()
}

fn query_resources<C: ResourcesComponent>(world: &World, id: &ResourceID) -> Vec<Entity> {
    world
        .query_with::<C>()
        .result()
        .into_iter()
        .filter(|entity| {
            world
                .get_component::<C>(*entity)
                .is_some_and(|component| component.resource_ids().any(|candidate| candidate == id))
        })
        .collect()
}

fn has_component<C: Component>(world: &World, entity: Entity) -> bool {
    world.get_component::<C>(entity).is_some()
}

pub trait WorldResourceExt {
    fn register_library<C: LibraryComponent>(
        &mut self,
        resource_type: &str,
    ) -> Result<(), ResourceError>;
    fn register_resources<C: ResourcesComponent>(
        &mut self,
        resource_type: &str,
    ) -> Result<(), ResourceError>;
    fn insert_library<C: LibraryComponent>(
        &mut self,
        entity: Entity,
        component: C,
    ) -> Result<(), ResourceError>;
    fn insert_resources<C: ResourcesComponent>(
        &mut self,
        entity: Entity,
        component: C,
    ) -> Result<(), ResourceError>;
    fn find_resource_entities(&self, id: &ResourceID) -> Result<Vec<Entity>, ResourceError>;
}

impl WorldResourceExt for World {
    fn register_library<C: LibraryComponent>(
        &mut self,
        resource_type: &str,
    ) -> Result<(), ResourceError> {
        register_library_type::<C>(self, resource_type)
    }
    fn register_resources<C: ResourcesComponent>(
        &mut self,
        resource_type: &str,
    ) -> Result<(), ResourceError> {
        register_resources_type::<C>(self, resource_type)
    }
    fn insert_library<C: LibraryComponent>(
        &mut self,
        entity: Entity,
        component: C,
    ) -> Result<(), ResourceError> {
        insert_library_component(self, entity, component)
    }
    fn insert_resources<C: ResourcesComponent>(
        &mut self,
        entity: Entity,
        component: C,
    ) -> Result<(), ResourceError> {
        insert_resources_component(self, entity, component)
    }
    fn find_resource_entities(&self, id: &ResourceID) -> Result<Vec<Entity>, ResourceError> {
        find_resource_entities(self, id)
    }
}
