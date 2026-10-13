use std::collections::BTreeMap;

use core_plugin::Component;

use crate::ResourceID;

pub trait ResourceInner: Send + Sync + 'static {
    fn get_alias(&self) -> &str;
}

pub trait LibraryComponent: Component {
    fn resource_id(&self) -> &ResourceID;
}

pub trait ResourcesComponent: Component {
    type Inner: Send + Sync + 'static;

    fn list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
    where
        Self::Inner: ResourceInner;

    fn get_by_id(&self, id: &ResourceID) -> Option<&Self::Inner>
    where
        Self::Inner: ResourceInner;

    fn resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_;
}

#[derive(Clone, Debug)]
pub struct ResourceSet<T: Send + Sync + 'static> {
    entries: BTreeMap<ResourceID, T>,
}

impl<T: Send + Sync + 'static> ResourceSet<T> {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, id: ResourceID, value: T) -> Option<T> {
        self.entries.insert(id, value)
    }

    pub fn list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
    where
        T: ResourceInner,
    {
        self.entries.keys()
    }

    pub fn get_by_id(&self, id: &ResourceID) -> Option<&T>
    where
        T: ResourceInner,
    {
        self.entries.get(id)
    }

    pub fn resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_ {
        self.entries.keys()
    }

    pub fn get(&self, id: &ResourceID) -> Option<&T> {
        self.entries.get(id)
    }

    pub fn get_mut(&mut self, id: &ResourceID) -> Option<&mut T> {
        self.entries.get_mut(id)
    }

    pub fn remove(&mut self, id: &ResourceID) -> Option<T> {
        self.entries.remove(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ResourceID, &T)> + '_ {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<T: Send + Sync + 'static> Default for ResourceSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Sync + 'static> Component for ResourceSet<T> {}

impl<T: Send + Sync + 'static> ResourcesComponent for ResourceSet<T> {
    type Inner = T;

    fn list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
    where
        T: ResourceInner,
    {
        self.entries.keys()
    }

    fn get_by_id(&self, id: &ResourceID) -> Option<&T>
    where
        T: ResourceInner,
    {
        self.entries.get(id)
    }

    fn resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_ {
        self.entries.keys()
    }
}
