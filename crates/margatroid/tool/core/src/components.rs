use crate::sync::update_task;
use crate::types::lock;
use crate::{TaskReadResult, ToolError, ToolErrorKind, ToolTaskHandle, ToolTaskSnapshot};
use crate::{TaskSyncBinding, TaskUpdateContext};
use core_plugin::Component;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, atomic::AtomicU64};

#[derive(Clone, Debug)]
pub struct ToolTasks {
    shared: Arc<TaskSetShared>,
    updates: TaskUpdateContext,
}

impl ToolTasks {
    pub fn new(owner: core_plugin::Entity, address: String, binding: TaskSyncBinding) -> Self {
        let shared = Arc::new(TaskSetShared {
            entries: Mutex::default(),
            gate: Arc::default(),
            revision: Arc::new(AtomicU64::new(0)),
            retire: Mutex::new(None),
        });
        let updates = binding.bind(owner, address, Arc::downgrade(&shared));
        let retirement = updates.clone();
        *lock(&shared.retire) = Some(Box::new(move || retirement.close()));
        Self { shared, updates }
    }
    pub fn update_context(&self) -> TaskUpdateContext {
        self.updates.clone()
    }
    pub fn close_sync(&self) {
        self.updates.close();
    }

    pub fn register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError> {
        update_task(&self.updates, || {
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
            if !self.updates.same_owner(handle.update_context()) {
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
    pub fn abort_unreturned(&self) -> usize {
        self.list()
            .into_iter()
            .filter(|(_, handle)| handle.abort_unreturned())
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
        if !self.updates.same_owner(context) {
            return update_task(context, || {
                handle.complete_cleanup_raw(error);
                Ok((false, false))
            })
            .unwrap_or(false);
        }
        update_task(&self.updates, || {
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

impl Component for ToolTasks {}

pub(crate) struct TaskSetShared {
    pub(crate) entries: Mutex<BTreeMap<String, ToolTaskHandle>>,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) revision: Arc<AtomicU64>,
    retire: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
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
