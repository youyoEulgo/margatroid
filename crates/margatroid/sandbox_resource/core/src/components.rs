use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use core_plugin::Component;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sandboxes {
    entries: BTreeMap<String, PathBuf>,
}

impl Sandboxes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, alias: &str) -> Option<&Path> {
        self.entries.get(alias).map(PathBuf::as_path)
    }

    pub fn insert(&mut self, alias: String, path: PathBuf) -> Option<PathBuf> {
        self.entries.insert(alias, path)
    }

    pub fn remove(&mut self, alias: &str) -> Option<PathBuf> {
        self.entries.remove(alias)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &PathBuf)> {
        self.entries.iter()
    }
}

impl Component for Sandboxes {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectedSandbox {
    pub alias: Option<String>,
}

impl Component for SelectedSandbox {}
