use crate::{Workspace, WorkspaceError, WorkspaceErrorKind, WorkspaceState};
use core_plugin::{Entity, Resource, World};
use std::sync::Arc;
type WorkspacePublisher = Arc<dyn Fn(&mut World, Entity) + Send + Sync>;
#[derive(Clone)]
pub struct WorkspaceUpdateContext(WorkspacePublisher);
impl Resource for WorkspaceUpdateContext {}
impl std::fmt::Debug for WorkspaceUpdateContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkspaceUpdateContext")
    }
}
impl WorkspaceUpdateContext {
    pub fn new(publish: impl Fn(&mut World, Entity) + Send + Sync + 'static) -> Self {
        Self(Arc::new(publish))
    }
    pub fn notify(&self, world: &mut World, owner: Entity) {
        (self.0)(world, owner)
    }
}
pub enum WorkspaceChange {
    State(WorkspaceState),
    AddMember(Entity),
    Members(std::collections::BTreeSet<Entity>),
    RemoveMember(Entity),
    Failure(Option<WorkspaceError>),
    Definition(crate::WorkspaceDefinition),
}
pub fn update_workspace(
    world: &mut World,
    owner: Entity,
    change: WorkspaceChange,
) -> Result<(), WorkspaceError> {
    let workspace = world
        .get_component_mut::<Workspace>(owner)
        .ok_or_else(|| WorkspaceError::new(WorkspaceErrorKind::Missing, "workspace missing"))?;
    let changed = match change {
        WorkspaceChange::State(value) => {
            let changed = workspace.data.state != value;
            workspace.data.state = value;
            changed
        }
        WorkspaceChange::AddMember(value) => workspace.data.agents.insert(value),
        WorkspaceChange::Members(value) => {
            let changed = workspace.data.agents != value;
            workspace.data.agents = value;
            changed
        }
        WorkspaceChange::RemoveMember(value) => workspace.data.agents.remove(&value),
        WorkspaceChange::Failure(value) => {
            let changed = workspace.data.error != value;
            workspace.data.error = value;
            changed
        }
        WorkspaceChange::Definition(value) => {
            let changed = workspace.data.definition.as_ref() != Some(&value);
            workspace.data.name = value.name.clone();
            workspace.data.project_root = value.project_root.clone();
            workspace.data.definition = Some(value);
            changed
        }
    };
    let context = workspace.updates.clone();
    if changed {
        context.notify(world, owner);
    }
    Ok(())
}
pub fn remove_workspace(world: &mut World, owner: Entity) {
    let context = world
        .get_component::<Workspace>(owner)
        .map(|item| item.updates.clone());
    world.despawn(owner);
    if let Some(context) = context {
        context.notify(world, owner);
    }
}
