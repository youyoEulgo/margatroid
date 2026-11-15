use crate::{ToolTaskHandle, ToolTasks};
use core_plugin::{Component, Entity};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, mpsc};
#[derive(Clone)]
pub struct LuaExecutionTask {
    pub workspace: Entity,
    pub handle: ToolTaskHandle,
    pub tasks: ToolTasks,
    pub input: Arc<Mutex<mpsc::UnboundedReceiver<Value>>>,
    pub resource_dir: PathBuf,
}
impl Component for LuaExecutionTask {}
