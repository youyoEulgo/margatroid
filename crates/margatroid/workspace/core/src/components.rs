use crate::{WorkspaceDatabase, WorkspaceState};
use core_plugin::Component;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub project_root: PathBuf,
    pub state: WorkspaceState,
    pub database: WorkspaceDatabase,
}
impl Workspace {
    pub fn new(project_root: PathBuf, database: WorkspaceDatabase) -> Self {
        Self {
            project_root,
            state: WorkspaceState::Running,
            database,
        }
    }
    /// Returns whether the state actually changed. Publish through the installed
    /// WorkspaceUpdateContext after releasing the component borrow.
    pub fn set_state(&mut self, state: WorkspaceState) -> bool {
        if self.state == state {
            return false;
        }
        self.state = state;
        true
    }
}
impl Component for Workspace {}
