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
    pub fn same_database(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.database.shared, &other.database.shared)
    }
}
