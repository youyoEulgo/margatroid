use crate::{ToolError, ToolTaskSnapshot};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentTaskSnapshot {
    pub address: String,
    pub tasks: Vec<ToolTaskSnapshot>,
}
pub type TaskChangeObserver = Arc<dyn Fn(u64) + Send + Sync>;
pub type TaskRetireObserver = Box<dyn Fn() + Send + Sync>;
pub type TaskSnapshotSource = Arc<dyn Fn() -> Option<(u64, Vec<ToolTaskSnapshot>)> + Send + Sync>;
#[derive(Clone)]
pub struct TaskUpdateContext {
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) revision: Arc<AtomicU64>,
    pub(crate) changed: Arc<Mutex<Option<TaskChangeObserver>>>,
}
impl std::fmt::Debug for TaskUpdateContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskUpdateContext").finish_non_exhaustive()
    }
}
impl TaskUpdateContext {
    pub fn same_collection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
    }
    pub fn commit(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn publish(&self, revision: u64) {
        let observer = crate::types::lock(&self.changed).clone();
        if let Some(observer) = observer {
            observer(revision);
        }
    }
}
pub(crate) fn update_task<R>(
    context: &TaskUpdateContext,
    change: impl FnOnce() -> Result<(R, bool), ToolError>,
) -> Result<R, ToolError> {
    let gate = crate::types::lock(&context.gate);
    let (result, changed) = change()?;
    let revision = changed.then(|| context.commit());
    drop(gate);
    if let Some(revision) = revision {
        context.publish(revision);
    }
    Ok(result)
}
