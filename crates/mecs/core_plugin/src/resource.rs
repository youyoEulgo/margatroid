use std::any::{Any, TypeId};
use std::collections::HashMap;

pub trait Resource: Send + Sync + 'static {}

pub(crate) struct ResourceRegistry {
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ResourceRegistry {
    pub(crate) fn new() -> Self {
        Self {
            resources: HashMap::new(),
        }
    }

    pub(crate) fn insert<R: Resource>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    pub(crate) fn get<R: Resource>(&self) -> Option<&R> {
        self.resources.get(&TypeId::of::<R>())?.downcast_ref()
    }

    pub(crate) fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources.get_mut(&TypeId::of::<R>())?.downcast_mut()
    }

    pub(crate) fn remove<R: Resource>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())?
            .downcast::<R>()
            .ok()
            .map(|resource| *resource)
    }

    pub(crate) fn contains<R: Resource>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }
}
