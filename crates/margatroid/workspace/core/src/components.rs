use crate::{WorkspaceExecutionClose, WorkspaceExecutionEntry, WorkspaceState};
use core_plugin::{Component, Entity};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Workspace {
    pub(crate) data: WorkspaceData,
    pub(crate) updates: crate::WorkspaceUpdateContext,
}
#[derive(Clone, Debug)]
pub struct WorkspaceData {
    pub name: String,
    pub definition_path: PathBuf,
    pub project_root: PathBuf,
    pub state: WorkspaceState,
    pub agents: BTreeSet<Entity>,
    pub definition: Option<crate::WorkspaceDefinition>,
    pub error: Option<crate::WorkspaceError>,
}
impl std::ops::Deref for Workspace {
    type Target = WorkspaceData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl Workspace {
    pub fn new(
        name: String,
        definition_path: PathBuf,
        project_root: PathBuf,
        state: WorkspaceState,
        agents: BTreeSet<Entity>,
        definition: Option<crate::WorkspaceDefinition>,
        updates: crate::WorkspaceUpdateContext,
    ) -> Self {
        Self {
            data: WorkspaceData {
                name,
                definition_path,
                project_root,
                state,
                agents,
                definition,
                error: None,
            },
            updates,
        }
    }
    pub fn updates(&self) -> crate::WorkspaceUpdateContext {
        self.updates.clone()
    }
}
impl Component for Workspace {}
#[derive(Debug)]
pub struct WorkspaceExecutor {
    pub entry: WorkspaceExecutionEntry,
    pub close: WorkspaceExecutionClose,
}
impl Component for WorkspaceExecutor {}
impl Drop for WorkspaceExecutor {
    fn drop(&mut self) {
        self.entry.close_admission();
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceFailure(pub crate::WorkspaceError);
impl Component for WorkspaceFailure {}
