use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use core_plugin::Component;

#[derive(Clone)]
pub struct Sandboxes {
    entries: BTreeMap<String, PathBuf>,
    updates: crate::SandboxChanged,
}

impl Sandboxes {
    pub fn new(updates: crate::SandboxChanged) -> Self {
        Self {
            entries: BTreeMap::new(),
            updates,
        }
    }
    pub fn publish_current(&self) {
        (self.updates)(
            self.entries
                .iter()
                .map(|(alias, path)| (alias.clone(), path.clone()))
                .collect(),
        );
    }

    pub fn get(&self, alias: &str) -> Option<&Path> {
        self.entries.get(alias).map(PathBuf::as_path)
    }

    pub fn insert(&mut self, alias: String, path: PathBuf) -> Option<PathBuf> {
        let changed = self.entries.get(&alias) != Some(&path);
        let previous = self.entries.insert(alias, path);
        if changed {
            self.publish_current();
        }
        previous
    }

    pub fn remove(&mut self, alias: &str) -> Option<PathBuf> {
        let previous = self.entries.remove(alias);
        if previous.is_some() {
            self.publish_current();
        }
        previous
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &PathBuf)> {
        self.entries.iter()
    }
}

impl Component for Sandboxes {}

#[derive(Clone)]
pub struct SelectedSandbox {
    data: SelectedSandboxData,
    updates: crate::SandboxSelectionChanged,
}

impl Component for SelectedSandbox {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedSandboxData {
    pub alias: Option<String>,
}
impl std::ops::Deref for SelectedSandbox {
    type Target = SelectedSandboxData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl SelectedSandbox {
    pub fn new(alias: Option<String>, updates: crate::SandboxSelectionChanged) -> Self {
        Self {
            data: SelectedSandboxData { alias },
            updates,
        }
    }
    pub fn set(&mut self, alias: Option<String>) {
        if self.data.alias != alias {
            self.data.alias = alias;
            (self.updates)(self.data.alias.clone());
        }
    }
    pub fn publish_current(&self) {
        (self.updates)(self.data.alias.clone());
    }
}
impl PartialEq for Sandboxes {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}
impl Eq for Sandboxes {}
impl PartialEq for SelectedSandbox {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}
impl Eq for SelectedSandbox {}

impl std::fmt::Debug for Sandboxes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandboxes")
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for SelectedSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedSandbox")
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}
