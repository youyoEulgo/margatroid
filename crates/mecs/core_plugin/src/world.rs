use std::sync::{RwLock, RwLockWriteGuard};

use crate::component::ComponentRegistry;
use crate::entity::EntityAllocator;
use crate::events::EventReadStorageRegistry;
use crate::query::Query;
use crate::resource::ResourceRegistry;
use crate::{Component, Entity, Event, EventQueue, EventReader, QueryResult, Resource};

pub struct World {
    entities: EntityAllocator,
    components: ComponentRegistry,
    resources: ResourceRegistry,
    query: Query,
    event_queue: RwLock<EventQueue>,
    event_registry: EventReadStorageRegistry,
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: EntityAllocator::new(),
            components: ComponentRegistry::new(),
            resources: ResourceRegistry::new(),
            query: Query::new(),
            event_queue: RwLock::new(EventQueue::new()),
            event_registry: EventReadStorageRegistry::new(),
        }
    }

    pub fn spawn(&mut self) -> Entity {
        self.entities.allocate()
    }

    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.is_alive(entity) {
            return false;
        }
        self.components.remove_entity(entity);
        self.entities.release(entity)
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub(crate) fn entity_iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter_alive()
    }

    pub fn insert_component<C: Component>(&mut self, entity: Entity, component: C) -> bool {
        if !self.entities.is_alive(entity) {
            return false;
        }
        self.components.insert(entity, component);
        true
    }

    pub fn get_component<C: Component>(&self, entity: Entity) -> Option<&C> {
        self.entities
            .is_alive(entity)
            .then(|| self.components.get(entity))
            .flatten()
    }

    pub fn get_component_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C> {
        self.entities
            .is_alive(entity)
            .then(|| self.components.get_mut(entity))
            .flatten()
    }

    pub fn remove_component<C: Component>(&mut self, entity: Entity) -> Option<C> {
        self.entities
            .is_alive(entity)
            .then(|| self.components.remove(entity))
            .flatten()
    }

    pub fn contains_component<C: Component>(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity) && self.components.contains::<C>(entity)
    }

    pub(crate) fn query_iter<C: Component>(&self) -> impl Iterator<Item = (&Entity, &C)> {
        self.components.iter::<C>()
    }

    pub fn query_with<C: Component>(&self) -> QueryResult<'_> {
        self.query.with::<C>(self)
    }

    pub fn query_without<C: Component>(&self) -> QueryResult<'_> {
        self.query.without::<C>(self)
    }

    pub fn insert_resource<R: Resource>(&mut self, resource: R) {
        self.resources.insert(resource);
    }

    pub fn get_resource<R: Resource>(&self) -> Option<&R> {
        self.resources.get()
    }

    pub fn get_resource_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources.get_mut()
    }

    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        self.resources.remove()
    }

    pub fn contains_resource<R: Resource>(&self) -> bool {
        self.resources.contains::<R>()
    }

    pub fn event_write(&self) -> RwLockWriteGuard<'_, EventQueue> {
        self.event_queue.write().expect("event queue lock poisoned")
    }

    pub(crate) fn event_registry_mut(&mut self) -> &mut EventReadStorageRegistry {
        &mut self.event_registry
    }

    pub fn event_reader<E: Event>(&self) -> EventReader<'_, E> {
        self.event_registry.reader()
    }

    pub(crate) fn tick(&mut self) {
        let mut queue = self.event_queue.write().expect("event queue lock poisoned");
        self.event_registry.clear();
        queue.pull_events(&mut self.event_registry);
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Health(u32);
    impl Component for Health {}

    struct Selected;
    impl Component for Selected {}

    struct Config(u32);
    impl Resource for Config {}

    #[test]
    fn entity_component_and_resource_operations_round_trip() {
        let mut world = World::new();
        let entity = world.spawn();
        assert!(world.insert_component(entity, Health(10)));
        world.insert_resource(Config(3));

        world.get_component_mut::<Health>(entity).unwrap().0 += 1;
        world.get_resource_mut::<Config>().unwrap().0 += 1;

        assert_eq!(world.get_component::<Health>(entity), Some(&Health(11)));
        assert_eq!(world.get_resource::<Config>().unwrap().0, 4);
        assert!(world.despawn(entity));
        assert!(!world.is_alive(entity));
        assert!(world.get_component::<Health>(entity).is_none());
    }

    #[test]
    fn query_filters_entities_and_releases_the_world_borrow() {
        let mut world = World::new();
        let selected = world.spawn();
        world.insert_component(selected, Health(1));
        world.insert_component(selected, Selected);
        let other = world.spawn();
        world.insert_component(other, Health(2));

        let entities = world.query_with::<Health>().with::<Selected>().result();
        world.get_component_mut::<Health>(entities[0]).unwrap().0 = 9;

        assert_eq!(entities, [selected]);
        assert_eq!(world.get_component::<Health>(selected), Some(&Health(9)));
    }

    #[test]
    fn query_without_starts_from_all_alive_entities() {
        let mut world = World::new();
        let with_health = world.spawn();
        world.insert_component(with_health, Health(1));
        let without_health = world.spawn();

        assert_eq!(world.query_without::<Health>().result(), [without_health]);
    }
}
