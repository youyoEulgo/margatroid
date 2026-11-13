use crate::UpdateProbe;
use core_plugin::{Entity, Resource};
use std::sync::Arc;
pub struct ProbeFactory<T> {
    callback: Arc<dyn Fn(Entity) -> UpdateProbe<T> + Send + Sync>,
}
impl<T> Clone for ProbeFactory<T> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone(),
        }
    }
}
impl<T: 'static + Send + Sync> Resource for ProbeFactory<T> {}
impl<T> ProbeFactory<T> {
    pub fn new(callback: impl Fn(Entity) -> UpdateProbe<T> + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }
    pub fn for_owner(&self, owner: Entity) -> UpdateProbe<T> {
        (self.callback)(owner)
    }
}
