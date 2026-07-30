use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::Entity;

pub trait Component: Send + Sync + 'static {}

trait ErasedSparseColumn: Any + Send + Sync + 'static {
    fn remove_entity(&mut self, entity: Entity);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub(crate) struct SparseColumn<C: Component> {
    sparse: Vec<Option<usize>>,
    entities: Vec<Entity>,
    components: Vec<C>,
}

impl<C: Component> SparseColumn<C> {
    pub(crate) fn new() -> Self {
        Self {
            sparse: Vec::new(),
            entities: Vec::new(),
            components: Vec::new(),
        }
    }

    fn dense_index(&self, entity: Entity) -> Option<usize> {
        let dense = self
            .sparse
            .get(entity.index() as usize)
            .copied()
            .flatten()?;
        (self.entities[dense].generation() == entity.generation()).then_some(dense)
    }

    pub(crate) fn insert(&mut self, entity: Entity, component: C) {
        if let Some(dense) = self.dense_index(entity) {
            self.components[dense] = component;
            return;
        }

        let sparse_index = entity.index() as usize;
        if sparse_index >= self.sparse.len() {
            self.sparse.resize(sparse_index + 1, None);
        }
        self.sparse[sparse_index] = Some(self.entities.len());
        self.entities.push(entity);
        self.components.push(component);
    }

    pub(crate) fn get(&self, entity: Entity) -> Option<&C> {
        self.components.get(self.dense_index(entity)?)
    }

    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut C> {
        let dense = self.dense_index(entity)?;
        self.components.get_mut(dense)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Entity, &C)> {
        self.entities.iter().zip(&self.components)
    }

    pub(crate) fn remove(&mut self, entity: Entity) -> Option<C> {
        let dense = self.dense_index(entity)?;
        self.sparse[entity.index() as usize] = None;
        self.entities.swap_remove(dense);
        let component = self.components.swap_remove(dense);
        if let Some(moved) = self.entities.get(dense) {
            self.sparse[moved.index() as usize] = Some(dense);
        }
        Some(component)
    }
}

impl<C: Component> ErasedSparseColumn for SparseColumn<C> {
    fn remove_entity(&mut self, entity: Entity) {
        self.remove(entity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct ComponentRegistry {
    columns: HashMap<TypeId, Box<dyn ErasedSparseColumn>>,
}

impl ComponentRegistry {
    pub(crate) fn new() -> Self {
        Self {
            columns: HashMap::new(),
        }
    }

    pub(crate) fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.columns
            .entry(TypeId::of::<C>())
            .or_insert_with(|| Box::new(SparseColumn::<C>::new()))
            .as_any_mut()
            .downcast_mut::<SparseColumn<C>>()
            .expect("component column type must match its TypeId")
            .insert(entity, component);
    }

    pub(crate) fn get<C: Component>(&self, entity: Entity) -> Option<&C> {
        self.column::<C>()?.get(entity)
    }

    pub(crate) fn get_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C> {
        self.column_mut::<C>()?.get_mut(entity)
    }

    pub(crate) fn iter<C: Component>(&self) -> impl Iterator<Item = (&Entity, &C)> {
        self.column::<C>().into_iter().flat_map(SparseColumn::iter)
    }

    pub(crate) fn remove<C: Component>(&mut self, entity: Entity) -> Option<C> {
        self.column_mut::<C>()?.remove(entity)
    }

    pub(crate) fn contains<C: Component>(&self, entity: Entity) -> bool {
        self.get::<C>(entity).is_some()
    }

    pub(crate) fn remove_entity(&mut self, entity: Entity) {
        for column in self.columns.values_mut() {
            column.remove_entity(entity);
        }
    }

    fn column<C: Component>(&self) -> Option<&SparseColumn<C>> {
        self.columns
            .get(&TypeId::of::<C>())?
            .as_any()
            .downcast_ref()
    }

    fn column_mut<C: Component>(&mut self) -> Option<&mut SparseColumn<C>> {
        self.columns
            .get_mut(&TypeId::of::<C>())?
            .as_any_mut()
            .downcast_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Health(u32);
    impl Component for Health {}

    #[test]
    fn removing_a_dense_value_repairs_the_sparse_mapping() {
        let first = Entity::new(1, 0);
        let second = Entity::new(9, 0);
        let mut column = SparseColumn::new();
        column.insert(first, Health(1));
        column.insert(second, Health(2));

        assert_eq!(column.remove(first), Some(Health(1)));
        assert_eq!(column.get(second), Some(&Health(2)));
    }

    #[test]
    fn registry_keeps_each_component_type_in_its_own_column() {
        struct Name;
        impl Component for Name {}

        let entity = Entity::new(0, 0);
        let mut registry = ComponentRegistry::new();
        registry.insert(entity, Health(10));
        registry.insert(entity, Name);

        assert_eq!(registry.get::<Health>(entity), Some(&Health(10)));
        assert!(registry.contains::<Name>(entity));
    }
}
