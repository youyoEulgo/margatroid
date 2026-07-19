use crate::component::Component;
use crate::entity::Entity;
use crate::world::World;
use std::marker::PhantomData;

/// 不可变查询。构造时收集匹配的 entity，iter() 从 World 按需取组件。
pub struct Query<'w, T: Component> {
    world: &'w World,
    entities: Vec<Entity>,
    _marker: PhantomData<T>,
}

impl<'w, T: Component> Query<'w, T> {
    /// 所有拥有 T 组件的 entity。
    pub fn all(world: &'w World) -> Self {
        let entities: Vec<Entity> = world.iter::<T>().map(|(e, _)| *e).collect();
        Query {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    /// 拥有 T 且拥有 C 的 entity。
    pub fn with<C: Component>(world: &'w World) -> Self {
        let entities: Vec<Entity> = world
            .iter::<T>()
            .filter(|(e, _)| world.has::<C>(**e))
            .map(|(e, _)| *e)
            .collect();
        Query {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    /// 拥有 T 但不拥有 C 的 entity。
    pub fn without<C: Component>(world: &'w World) -> Self {
        let entities: Vec<Entity> = world
            .iter::<T>()
            .filter(|(e, _)| !world.has::<C>(**e))
            .map(|(e, _)| *e)
            .collect();
        Query {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.entities.iter().filter_map(|e| self.world.get::<T>(*e))
    }

    pub fn single(&self) -> Option<&T> {
        self.iter().next()
    }

    pub fn entity_iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter().copied()
    }
}

/// 可变查询。构造时收集匹配的 entity，并独占借用 World。
pub struct QueryMut<'w, T: Component> {
    world: &'w mut World,
    entities: Vec<Entity>,
    _marker: PhantomData<&'w mut T>,
}

impl<'w, T: Component> QueryMut<'w, T> {
    pub fn all(world: &'w mut World) -> Self {
        let entities: Vec<Entity> = world.iter::<T>().map(|(e, _)| *e).collect();
        QueryMut {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    pub fn with<C: Component>(world: &'w mut World) -> Self {
        let entities: Vec<Entity> = world
            .iter::<T>()
            .filter(|(e, _)| world.has::<C>(**e))
            .map(|(e, _)| *e)
            .collect();
        QueryMut {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    pub fn without<C: Component>(world: &'w mut World) -> Self {
        let entities: Vec<Entity> = world
            .iter::<T>()
            .filter(|(e, _)| !world.has::<C>(**e))
            .map(|(e, _)| *e)
            .collect();
        QueryMut {
            world,
            entities,
            _marker: PhantomData,
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        let entities = &self.entities;
        self.world
            .iter_mut::<T>()
            .filter_map(move |(entity, component)| entities.contains(entity).then_some(component))
    }
}

/// 不可变资源引用。
pub struct Res<'w, R> {
    pub(crate) inner: &'w R,
}

impl<'w, R> Res<'w, R> {
    pub fn new(world: &'w World) -> Self
    where
        R: crate::resource::Resource,
    {
        Res {
            inner: world.resource::<R>().expect("resource not registered"),
        }
    }
}

impl<'w, R> std::ops::Deref for Res<'w, R> {
    type Target = R;
    fn deref(&self) -> &R {
        self.inner
    }
}

/// 可变资源引用。
pub struct ResMut<'w, R> {
    pub(crate) inner: &'w mut R,
}

impl<'w, R> ResMut<'w, R> {
    pub fn new(world: &'w mut World) -> Self
    where
        R: crate::resource::Resource,
    {
        ResMut {
            inner: world.resource_mut::<R>().expect("resource not registered"),
        }
    }
}

impl<'w, R> std::ops::Deref for ResMut<'w, R> {
    type Target = R;
    fn deref(&self) -> &R {
        self.inner
    }
}

impl<'w, R> std::ops::DerefMut for ResMut<'w, R> {
    fn deref_mut(&mut self) -> &mut R {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Health(i32);
    struct Selected;

    #[test]
    fn mutable_query_only_updates_matching_entities() {
        let mut world = World::new();
        let selected = world.spawn((Health(1), Selected));
        let other = world.spawn((Health(2),));

        let mut query = QueryMut::<Health>::with::<Selected>(&mut world);
        for health in query.iter_mut() {
            health.0 += 10;
        }
        drop(query);

        assert_eq!(world.get::<Health>(selected).unwrap().0, 11);
        assert_eq!(world.get::<Health>(other).unwrap().0, 2);
    }
}
