use crate::{Component, Entity, World};

pub struct QueryResult<'world> {
    world: &'world World,
    entities: Vec<Entity>,
}

impl QueryResult<'_> {
    pub fn with<C: Component>(mut self) -> Self {
        self.entities
            .retain(|entity| self.world.contains_component::<C>(*entity));
        self
    }

    pub fn without<C: Component>(mut self) -> Self {
        self.entities
            .retain(|entity| !self.world.contains_component::<C>(*entity));
        self
    }

    pub fn result(self) -> Vec<Entity> {
        self.entities
    }
}

pub(crate) struct Query;

impl Query {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn with<'world, C: Component>(&self, world: &'world World) -> QueryResult<'world> {
        QueryResult {
            world,
            entities: world.query_iter::<C>().map(|(entity, _)| *entity).collect(),
        }
    }

    pub(crate) fn without<'world, C: Component>(
        &self,
        world: &'world World,
    ) -> QueryResult<'world> {
        QueryResult {
            world,
            entities: world
                .entity_iter()
                .filter(|entity| !world.contains_component::<C>(*entity))
                .collect(),
        }
    }
}
