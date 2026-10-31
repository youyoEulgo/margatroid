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
