use crate::{ToolError, ToolErrorKind};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Clone, Default)]
pub struct TaskScope(Arc<ScopeInner>);
#[derive(Default)]
struct ScopeInner {
    state: Mutex<ScopeState>,
}
#[derive(Default)]
struct ScopeState {
    closed: bool,
    registrations: BTreeMap<u64, Box<dyn FnOnce() + Send>>,
}
impl std::fmt::Debug for TaskScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("TaskScope")
            .field("closed", &state.closed)
            .field("registrations", &state.registrations.len())
            .finish()
    }
}
pub struct TaskBinding {
    scope: Weak<ScopeInner>,
    id: u64,
}
impl TaskScope {
    pub fn bind(&self, revoke: impl FnOnce() + Send + 'static) -> Result<TaskBinding, ToolError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(ToolError::new(
                ToolErrorKind::TaskMissing,
                "task lifetime is closed",
            ));
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        state.registrations.insert(id, Box::new(revoke));
        Ok(TaskBinding {
            scope: Arc::downgrade(&self.0),
            id,
        })
    }
    pub fn close(&self) {
        let registrations = {
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return;
            }
            state.closed = true;
            std::mem::take(&mut state.registrations)
        };
        for (_, revoke) in registrations {
            revoke();
        }
    }
    pub fn availability(&self) -> Box<dyn Fn() -> bool + Send + Sync> {
        let weak = Arc::downgrade(&self.0);
        Box::new(move || {
            weak.upgrade()
                .is_some_and(|scope| !scope.state.lock().unwrap_or_else(|e| e.into_inner()).closed)
        })
    }
    pub fn is_closed(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
    }
}
impl Drop for TaskBinding {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.upgrade() {
            scope
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .registrations
                .remove(&self.id);
        }
    }
}
impl Drop for ScopeInner {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        for (_, revoke) in std::mem::take(&mut state.registrations) {
            revoke();
        }
    }
}
