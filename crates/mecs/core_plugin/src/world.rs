use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::component::{Bundle, Component};
use crate::entity::Entity;
use crate::events::{Event, EventReader, Events};
use crate::resource::Resource;

/// 稀疏集合列：entity index → 稠密槽位 → 组件数据。
struct Column {
    sparse: Vec<Option<usize>>,
    entities: Vec<Entity>,
    components: Vec<Box<dyn Any + Send + Sync>>,
}

impl Column {
    fn new() -> Self {
        Column {
            sparse: Vec::new(),
            entities: Vec::new(),
            components: Vec::new(),
        }
    }

    fn insert(&mut self, entity: Entity, component: Box<dyn Any + Send + Sync>) {
        let idx = entity.index() as usize;
        if idx >= self.sparse.len() {
            self.sparse.resize(idx + 1, None);
        }

        if let Some(dense) = self.get_dense(entity) {
            self.components[dense] = component;
            return;
        }

        let dense = self.entities.len();
        self.sparse[idx] = Some(dense);
        self.entities.push(entity);
        self.components.push(component);
    }

    fn get_dense(&self, entity: Entity) -> Option<usize> {
        let idx = entity.index() as usize;
        let dense = self.sparse.get(idx).copied().flatten()?;
        if self.entities[dense].generation() != entity.generation() {
            return None;
        }
        Some(dense)
    }

    fn remove(&mut self, entity: Entity) -> Option<Box<dyn Any + Send + Sync>> {
        let dense = self.get_dense(entity)?;
        let last = self.entities.len() - 1;
        // swap-remove：用最后一个元素填充空洞
        self.entities.swap(dense, last);
        self.components.swap(dense, last);
        // 修复被交换进来的 entity 的稀疏索引
        let swapped = self.entities[dense];
        self.sparse[swapped.index() as usize] = Some(dense);
        // 清除被删除 entity 的记录
        self.sparse[entity.index() as usize] = None;
        self.entities.pop();
        Some(self.components.pop().unwrap())
    }
}

pub struct World {
    /// 组件类型 → 列存储
    columns: HashMap<TypeId, Column>,
    /// 资源类型 → 全局单例
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// 回收的 entity index，等待复用
    free_indices: Vec<u32>,
    /// 每个 entity index 的当前 generation
    generations: Vec<u32>,
    /// 当前存活的 entity 数量
    entity_count: usize,
}

impl World {
    pub fn new() -> Self {
        World {
            columns: HashMap::new(),
            resources: HashMap::new(),
            free_indices: Vec::new(),
            generations: Vec::new(),
            entity_count: 0,
        }
    }

    /// 分配新 entity（优先复用回收的 index）。
    fn reserve_entity(&mut self) -> Entity {
        if let Some(index) = self.free_indices.pop() {
            let gen = self.generations[index as usize];
            self.entity_count += 1;
            Entity::new(index, gen)
        } else {
            let index = self.generations.len() as u32;
            self.generations.push(0);
            self.entity_count += 1;
            Entity::new(index, 0)
        }
    }

    /// 创建 entity 并挂载一组组件。
    pub fn spawn(&mut self, components: impl Bundle) -> Entity {
        let entity = self.reserve_entity();
        Box::new(components).apply(&mut |c: Box<dyn Any + Send + Sync>| {
            let type_id = (*c).type_id();
            let col = self.columns.entry(type_id).or_insert_with(Column::new);
            col.insert(entity, c);
        });
        entity
    }

    /// 删除 entity，bump generation 并回收 index。
    pub fn despawn(&mut self, entity: Entity) {
        let idx = entity.index() as usize;
        if idx >= self.generations.len() || self.generations[idx] != entity.generation() {
            return;
        }
        self.generations[idx] = self.generations[idx].wrapping_add(1);
        for col in self.columns.values_mut() {
            col.remove(entity);
        }
        self.free_indices.push(entity.index());
        self.entity_count -= 1;
    }

    /// 动态添加组件到已有 entity。
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
        let idx = entity.index() as usize;
        if idx >= self.generations.len() || self.generations[idx] != entity.generation() {
            return;
        }
        let col = self
            .columns
            .entry(TypeId::of::<T>())
            .or_insert_with(Column::new);
        col.insert(entity, Box::new(component));
    }

    /// 移除并返回组件。
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        let col = self.columns.get_mut(&TypeId::of::<T>())?;
        col.remove(entity).map(|b| *b.downcast::<T>().ok().unwrap())
    }

    /// 不可变访问组件。
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let col = self.columns.get(&TypeId::of::<T>())?;
        let dense = col.get_dense(entity)?;
        col.components[dense].downcast_ref::<T>()
    }

    /// 可变访问组件。
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        let col = self.columns.get_mut(&TypeId::of::<T>())?;
        let dense = col.get_dense(entity)?;
        col.components[dense].downcast_mut::<T>()
    }

    /// 遍历所有拥有 T 组件的 (Entity, &T)。
    pub fn iter<T: Component>(&self) -> impl Iterator<Item = (&Entity, &T)> {
        self.columns
            .get(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|col| {
                col.entities
                    .iter()
                    .zip(col.components.iter())
                    .filter_map(|(e, c)| Some((e, c.downcast_ref::<T>()?)))
            })
    }

    /// 遍历所有拥有 T 组件的 (Entity, &mut T)。
    pub fn iter_mut<T: Component>(&mut self) -> impl Iterator<Item = (&Entity, &mut T)> {
        self.columns
            .get_mut(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|col| {
                col.entities
                    .iter()
                    .zip(col.components.iter_mut())
                    .filter_map(|(e, c)| Some((e, c.downcast_mut::<T>()?)))
            })
    }

    /// 注册或替换全局资源。
    pub fn add_resource<R: Resource>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    /// 不可变访问资源。
    pub fn resource<R: Resource>(&self) -> Option<&R> {
        self.resources
            .get(&TypeId::of::<R>())
            .and_then(|r| r.downcast_ref::<R>())
    }

    /// 可变访问资源。
    pub fn resource_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())
            .and_then(|r| r.downcast_mut::<R>())
    }

    /// 移除并返回资源。
    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .and_then(|r| r.downcast::<R>().ok())
            .map(|b| *b)
    }

    /// 当前存活的 entity 数量。
    pub fn entity_count(&self) -> usize {
        self.entity_count
    }

    /// 检查 entity 是否仍然存活。
    pub fn is_alive(&self, entity: Entity) -> bool {
        let idx = entity.index() as usize;
        idx < self.generations.len() && self.generations[idx] == entity.generation()
    }

    /// 检查 entity 是否拥有指定组件类型。
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        self.get::<T>(entity).is_some()
    }

    /// 向已注册的事件队列发送事件。
    pub fn send_event<E: Event>(&self, event: E) {
        self.resource::<Events<E>>()
            .unwrap_or_else(|| panic!("event `{}` is not registered", std::any::type_name::<E>()))
            .send(event);
    }

    /// 读取该 reader 尚未消费的事件。
    pub fn read_events<E: Event>(&self, reader: &mut EventReader<E>) -> Vec<E> {
        self.resource::<Events<E>>()
            .unwrap_or_else(|| panic!("event `{}` is not registered", std::any::type_name::<E>()))
            .read(reader)
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
    struct Health(i32);
    #[derive(Debug, PartialEq)]
    struct Name(String);

    #[test]
    fn spawn_and_get() {
        let mut world = World::new();
        let e = world.spawn((Health(100), Name("alice".into())));
        assert_eq!(world.get::<Health>(e).unwrap().0, 100);
        assert_eq!(world.get::<Name>(e).unwrap().0, "alice");
    }

    #[test]
    fn despawn_and_reuse() {
        let mut world = World::new();
        let e1 = world.spawn((Health(1),));
        world.despawn(e1);
        assert!(!world.is_alive(e1));
        let e2 = world.spawn((Health(2),));
        // index 复用但 generation 不同
        assert_eq!(e2.index(), e1.index());
        assert_ne!(e2.generation(), e1.generation());
        assert_eq!(world.get::<Health>(e2).unwrap().0, 2);
    }

    #[test]
    fn iter_components() {
        let mut world = World::new();
        world.spawn((Health(10),));
        world.spawn((Health(20),));
        let mut values: Vec<i32> = world.iter::<Health>().map(|(_, h)| h.0).collect();
        values.sort();
        assert_eq!(values, vec![10, 20]);
    }

    #[test]
    fn resource_roundtrip() {
        let mut world = World::new();
        world.add_resource(Health(99));
        assert_eq!(world.resource::<Health>().unwrap().0, 99);
        world.resource_mut::<Health>().unwrap().0 = 55;
        assert_eq!(world.resource::<Health>().unwrap().0, 55);
        let removed = world.remove_resource::<Health>();
        assert_eq!(removed.unwrap().0, 55);
        assert!(world.resource::<Health>().is_none());
    }

    #[test]
    fn remove_component() {
        let mut world = World::new();
        let e = world.spawn((Health(1), Name("bob".into())));
        let removed = world.remove::<Health>(e);
        assert_eq!(removed.unwrap().0, 1);
        assert!(world.get::<Health>(e).is_none());
        assert!(world.get::<Name>(e).is_some());
    }

    #[test]
    fn inserting_same_component_replaces_existing_value() {
        let mut world = World::new();
        let e = world.spawn((Health(1),));

        world.insert(e, Health(2));

        assert_eq!(world.get::<Health>(e), Some(&Health(2)));
        assert_eq!(world.iter::<Health>().count(), 1);
        world.despawn(e);
        assert_eq!(world.iter::<Health>().count(), 0);
    }
}
