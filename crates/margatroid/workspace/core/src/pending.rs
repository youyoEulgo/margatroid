use core_plugin::Resource;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct PendingState {
    load_started: bool,
    load_completed: bool,
    creates: usize,
}

/// Tracks initialization and accepted creates until World has consumed their
/// StartWorkspace events. It never owns an ECS entity or a database.
#[derive(Clone, Default)]
pub struct WorkspacePendingWork {
    shared: Arc<Mutex<PendingState>>,
}
impl Resource for WorkspacePendingWork {}
impl std::fmt::Debug for WorkspacePendingWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspacePendingWork")
            .field("load_completed", &self.load_completed())
            .field("is_idle", &self.is_idle())
            .finish()
    }
}
impl WorkspacePendingWork {
    /// Exactly one startup load is admitted per installed plugin.
    pub fn begin_load(&self) -> Option<WorkspaceWorkToken> {
        let mut state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if state.load_started {
            return None;
        }
        state.load_started = true;
        Some(WorkspaceWorkToken {
            shared: self.shared.clone(),
            load: true,
        })
    }
    pub fn begin_create(&self) -> WorkspaceWorkToken {
        let mut state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        state.creates = state
            .creates
            .checked_add(1)
            .expect("Workspace create count overflow");
        WorkspaceWorkToken {
            shared: self.shared.clone(),
            load: false,
        }
    }
    pub fn load_completed(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .load_completed
    }
    pub fn is_idle(&self) -> bool {
        let state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        state.load_completed && state.creates == 0
    }
}

/// The plugin carries this token into its private completion marker, releasing
/// it only after start_workspace_system processed all earlier StartWorkspace
/// events. Dropping an asynchronous waiter must not release accepted work.
pub struct WorkspaceWorkToken {
    shared: Arc<Mutex<PendingState>>,
    load: bool,
}
impl Drop for WorkspaceWorkToken {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if self.load {
            state.load_completed = true;
        } else {
            state.creates = state
                .creates
                .checked_sub(1)
                .expect("Workspace create token underflow");
        }
    }
}
