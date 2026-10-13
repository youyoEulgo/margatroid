mod blocks;
mod database;
mod history;

use core_plugin::{App, Plugin, Resource};

pub use database::SqliteAgentDatabase;
pub use history::SqliteHistoryStore;
pub use margatroid_types::history_components::{
    AgentHistoryHandle, AgentHistoryStore, HistoryError, HistoryRecord,
};

#[derive(Default)]
pub struct MemoryPlugin;

impl MemoryPlugin {
    pub fn new() -> Self {
        Self
    }
}

pub struct MemoryPluginInstalled;

impl Resource for MemoryPluginInstalled {}

impl Plugin for MemoryPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<MemoryPluginInstalled>() {
            panic!("MemoryPlugin is already installed");
        }
        app.world_mut().insert_resource(MemoryPluginInstalled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_installs_without_runtime_or_agent_dependencies() {
        let mut app = App::new();
        app.add_plugin(MemoryPlugin::new());
        assert!(app.world().contains_resource::<MemoryPluginInstalled>());
    }

    #[test]
    #[should_panic(expected = "MemoryPlugin is already installed")]
    fn duplicate_plugin_installation_is_rejected() {
        let mut app = App::new();
        app.add_plugin(MemoryPlugin::new());
        app.add_plugin(MemoryPlugin::new());
    }
}
