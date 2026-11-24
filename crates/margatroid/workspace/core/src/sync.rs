use crate::WorkspaceUpdateKind;
use core_plugin::{Entity, Resource, World};
use std::sync::Arc;

type WorkspacePublisher = Arc<dyn Fn(&mut World, Entity, WorkspaceUpdateKind) + Send + Sync>;
#[derive(Clone)]
pub struct WorkspaceUpdateContext {
    publish: WorkspacePublisher,
}
impl WorkspaceUpdateContext {
    pub fn new(
        publish: impl Fn(&mut World, Entity, WorkspaceUpdateKind) + Send + Sync + 'static,
    ) -> Self {
        Self {
            publish: Arc::new(publish),
        }
    }
    pub fn notify(&self, world: &mut World, owner: Entity, kind: WorkspaceUpdateKind) {
        (self.publish)(world, owner, kind)
    }
}
impl Resource for WorkspaceUpdateContext {}
impl std::fmt::Debug for WorkspaceUpdateContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceUpdateContext")
            .finish_non_exhaustive()
    }
}
