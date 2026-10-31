use crate::types::lock;
use crate::{TaskReadResult, ToolError, ToolErrorKind, ToolTaskHandle};
use core_plugin::Component;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub struct ToolTasks {
    entries: Arc<Mutex<BTreeMap<String, ToolTaskHandle>>>,
}

impl ToolTasks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, id: String, handle: ToolTaskHandle) -> Result<(), ToolError> {
        if id.trim().is_empty() || id.chars().any(char::is_control) {
            return Err(ToolError::new(
                ToolErrorKind::InvalidRequest,
                "invalid tool task id",
            ));
        }
        let mut entries = lock(&self.entries);
        if entries.contains_key(&id) {
            return Err(ToolError::new(
                ToolErrorKind::DuplicateTask,
                "tool task id already registered",
            ));
        }
        entries.insert(id, handle);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<ToolTaskHandle, ToolError> {
        lock(&self.entries)
            .get(id)
            .cloned()
            .ok_or_else(|| ToolError::new(ToolErrorKind::TaskMissing, "tool task not found"))
    }

    pub fn cancel(&self, id: &str) -> Result<(), ToolError> {
        self.get(id)?.cancel();
        Ok(())
    }

    pub fn list(&self) -> Vec<(String, ToolTaskHandle)> {
        lock(&self.entries)
            .iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect()
    }

    pub fn read(&self, id: &str) -> Result<TaskReadResult, ToolError> {
        Ok(self.get(id)?.read())
    }

    pub fn send(&self, id: &str, value: Value) -> Result<(), ToolError> {
        self.get(id)?.send(value)
    }

    pub async fn finish(&self, id: &str) -> Result<(), ToolError> {
        self.get(id)?.finish().await
    }
}

impl Component for ToolTasks {}
