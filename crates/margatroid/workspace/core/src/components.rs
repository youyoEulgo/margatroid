use crate::{WorkspaceExecutionClose, WorkspaceExecutionEntry, WorkspaceState};
use core_plugin::{Component, Entity};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Workspace {
    pub name: String,
    pub definition_path: PathBuf,
    pub project_root: PathBuf,
    pub state: WorkspaceState,
    pub agents: BTreeSet<Entity>,
    pub definition: Option<crate::WorkspaceDefinition>,
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
