use crate::sync::update_task;
use crate::types::lock;
use crate::{TaskChangeObserver, TaskRetireObserver, TaskSnapshotSource, TaskUpdateContext};
use crate::{
    TaskListEntry, TaskReadResult, ToolError, ToolErrorKind, ToolTaskHandle, ToolTaskSnapshot,
};
use core_plugin::Component;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Debug)]
pub struct ToolTasks {
    shared: Arc<TaskSetShared>,
    updates: TaskUpdateContext,
}

impl ToolTasks {
    pub fn new() -> Self {
        let shared = Arc::new(TaskSetShared {
            entries: Mutex::default(),
            gate: Arc::default(),
            revision: Arc::new(AtomicU64::new(0)),
            retire: Mutex::new(None),
            accepting: AtomicBool::new(true),
        });
        let updates = TaskUpdateContext {
            gate: shared.gate.clone(),
            revision: shared.revision.clone(),
            changed: Arc::default(),
        };
        Self { shared, updates }
    }
    pub fn update_context(&self) -> TaskUpdateContext {
        self.updates.clone()
    }
    pub fn close_admission(&self) {
        let _gate = lock(&self.shared.gate);
        self.shared.accepting.store(false, Ordering::Release);
    }
    pub fn is_accepting(&self) -> bool {
        self.shared.accepting.load(Ordering::Acquire)
    }
    pub fn observe(&self, changed: TaskChangeObserver, retired: TaskRetireObserver) {
        *lock(&self.updates.changed) = Some(changed);
        let previous = lock(&self.shared.retire).replace(retired);
        if let Some(previous) = previous {
            previous();
        }
    }
    pub fn snapshot_source(&self) -> TaskSnapshotSource {
        let weak = Arc::downgrade(&self.shared);
        Arc::new(move || {
            let shared = weak.upgrade()?;
            let _gate = lock(&shared.gate);
            let tasks = lock(&shared.entries)
                .iter()
                .filter_map(|(id, handle)| handle.snapshot(id))
                .collect();
            Some((shared.revision.load(Ordering::Acquire), tasks))
        })
    }

    pub fn register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError> {
        update_task(&self.updates, || {
            if !self.is_accepting() {
                return Err(ToolError::new(
                    ToolErrorKind::Closed,
                    "task collection is closed",
                ));
            }
            if id.trim().is_empty() || id.chars().any(char::is_control) {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "invalid tool task id",
                ));
            }
            let mut entries = lock(&self.shared.entries);
            if handle.is_cleanup_complete() {
                return Err(ToolError::new(
                    ToolErrorKind::TaskMissing,
                    "tool task has been cleaned up",
                ));
            }
            if entries.contains_key(&id) {
                return Err(ToolError::new(
                    ToolErrorKind::DuplicateTask,
                    "tool task id already registered",
                ));
            }
            if !self.updates.same_collection(handle.update_context()) {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidRequest,
                    "task belongs to another update context",
                ));
            }
            entries.insert(id, handle);
            Ok(((), true))
        })
    }

    pub fn get(&self, id: &str) -> Result<ToolTaskHandle, ToolError> {
        lock(&self.shared.entries)
            .get(id)
            .cloned()
            .ok_or_else(|| ToolError::new(ToolErrorKind::TaskMissing, "tool task not found"))
    }

    pub fn cancel(&self, id: &str) -> Result<(), ToolError> {
        self.get(id)?.cancel();
        Ok(())
    }

    pub fn list(&self) -> Vec<(String, ToolTaskHandle)> {
        lock(&self.shared.entries)
            .iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect()
    }

    pub fn list_returned(&self) -> Vec<TaskListEntry> {
        let _gate = lock(&self.shared.gate);
        lock(&self.shared.entries)
            .iter()
            .filter_map(|(id, handle)| handle.list_entry(id))
            .collect()
    }

    pub fn snapshot(&self) -> Vec<ToolTaskSnapshot> {
        let _gate = lock(&self.shared.gate);
        self.list()
            .into_iter()
            .filter_map(|(id, handle)| handle.snapshot(&id))
            .collect()
    }

    pub fn read(&self, id: &str) -> Result<TaskReadResult, ToolError> {
        self.get(id)?.read()
    }

    pub fn send(&self, id: &str, value: Value) -> Result<(), ToolError> {
        self.get(id)?.send(value)
    }

    pub async fn finish(&self, id: &str) -> Result<(), ToolError> {
        self.get(id)?.finish().await
    }

    /// Synchronously deliver aborted replies and request cancellation only for pending calls.
    pub fn abort_unreturned(&self, turn: &str) -> usize {
        self.list()
            .into_iter()
            .filter(|(_, handle)| handle.turn() == Some(turn) && handle.abort_unreturned())
            .count()
    }

    /// Called by the supervisor only after all owned resources have been reclaimed.
    /// Keep the collection lock through removal and completion so new queries cannot
    /// observe a completed entry, and never remove a newer task reusing the id.
    pub fn complete_cleanup(
        &self,
        id: &str,
        handle: &ToolTaskHandle,
        error: Option<ToolError>,
    ) -> bool {
        let context = handle.update_context();
        if !self.updates.same_collection(context) {
            return update_task(context, || {
                if !handle.begin_cleanup_completion() {
                    return Ok((false, false));
                }
                handle.complete_cleanup_raw(error);
                Ok((false, false))
            })
            .unwrap_or(false);
        }
        update_task(&self.updates, || {
            if !handle.begin_cleanup_completion() {
                return Ok((false, false));
            }
            let mut entries = lock(&self.shared.entries);
            let current = entries
                .get(id)
                .is_some_and(|current| current.same_task(handle));
            if current {
                entries.remove(id);
            }
            handle.complete_cleanup_raw(error);
            Ok((current, current))
        })
        .unwrap_or(false)
    }
}

impl Default for ToolTasks {
    fn default() -> Self {
        Self::new()
    }
}
impl Component for ToolTasks {}

pub(crate) struct TaskSetShared {
    pub(crate) entries: Mutex<BTreeMap<String, ToolTaskHandle>>,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) revision: Arc<AtomicU64>,
    retire: Mutex<Option<TaskRetireObserver>>,
    accepting: AtomicBool,
}

impl std::fmt::Debug for TaskSetShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskSetShared")
            .field("tasks", &lock(&self.entries).len())
            .finish()
    }
}
impl Drop for TaskSetShared {
    fn drop(&mut self) {
        if let Some(retire) = self
            .retire
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            retire();
        }
    }
}
