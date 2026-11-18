use crate::StorageDatabase;
use core_plugin::Component;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct AgentStorage {
    pub(crate) database: StorageDatabase,
    pub(crate) settings_path: PathBuf,
    pub(crate) settings_lock: Arc<Mutex<()>>,
}
impl Component for AgentStorage {}

impl AgentStorage {
    pub fn data_dir(&self) -> &std::path::Path {
        self.settings_path
            .parent()
            .expect("Agent settings have an absolute parent")
    }
    pub fn same_database(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.database.shared, &other.database.shared)
    }
}
