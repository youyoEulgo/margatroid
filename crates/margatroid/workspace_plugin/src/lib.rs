mod error;
mod events;
mod handler;
mod system;
mod types;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_image_loader_plugin::AgentImageLoaderPluginInstalled;
use agent_plugin::AgentPluginInstalled;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Plugin, World};
use lua_runtime_plugin::LuaRuntimePluginInstalled;
use margatroid_types::{ResourceId, StartWorkspace, WorkspaceDefinition};
use mcl_plugin::MclPluginInstalled;
use memory_plugin::MemoryPluginInstalled;
use resource_id_plugin::ResourceIdPluginInstalled;
use resource_id_plugin::WorldResourceIdExt;
use tool_plugin::ToolPluginInstalled;

pub use error::{WorkspaceError, WorkspaceErrorKind};
pub use events::*;
use system::{
    begin_workspace_command_system, collect_agent_image_system,
    collect_agent_initialization_system, collect_mcl_command_response_system,
    route_agent_message_system, route_agent_turn_abort_system, route_mcl_command_system,
};
pub use types::{WorkspaceAgentState, WorkspaceRegistry};

#[derive(Clone, Debug)]
pub struct Workspace {
    pub(crate) definition: Arc<WorkspaceDefinition>,
    pub(crate) project_root: Arc<PathBuf>,
    pub(crate) manager_name: String,
    pub(crate) agents: BTreeMap<String, Entity>,
    pub(crate) states: BTreeMap<String, WorkspaceAgentState>,
}

impl Workspace {
    pub fn definition(&self) -> &WorkspaceDefinition {
        &self.definition
    }

    pub fn project_root(&self) -> &Path {
        self.project_root.as_path()
    }

    pub fn manager(&self) -> Option<Entity> {
        self.agents.get(&self.manager_name).copied()
    }

    pub fn agent(&self, name: &str) -> Option<Entity> {
        self.agents.get(name).copied()
    }

    pub fn state(&self, name: &str) -> Option<&WorkspaceAgentState> {
        self.states.get(name)
    }

    pub fn states(&self) -> impl Iterator<Item = (&str, &WorkspaceAgentState)> + '_ {
        self.states
            .iter()
            .map(|(name, state)| (name.as_str(), state))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_ {
        self.agents
            .iter()
            .map(|(name, entity)| (name.as_str(), *entity))
    }
}

impl Component for Workspace {}

pub struct WorkspacePlugin {
    agent_images_root: PathBuf,
    schedule: String,
}

impl WorkspacePlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidAgentImagesRoot,
                "image root is empty",
            ));
        }
        Ok(Self {
            agent_images_root: root,
            schedule: RuntimePlugin::UPDATE.into(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for WorkspacePlugin {
    fn default() -> Self {
        Self {
            agent_images_root: PathBuf::new(),
            schedule: RuntimePlugin::UPDATE.into(),
        }
    }
}

impl Plugin for WorkspacePlugin {
    fn build(self, app: &mut App) {
        if !app.contains_schedule(&self.schedule) {
            panic!("WorkspacePlugin schedule does not exist");
        }
        if !app.world().contains_resource::<ResourceIdPluginInstalled>()
            || !app
                .world()
                .contains_resource::<AgentImageLoaderPluginInstalled>()
            || !app.world().contains_resource::<AgentPluginInstalled>()
            || !app.world().contains_resource::<ToolPluginInstalled>()
            || !app.world().contains_resource::<MemoryPluginInstalled>()
            || !app.world().contains_resource::<MclPluginInstalled>()
            || !app.world().contains_resource::<LuaRuntimePluginInstalled>()
        {
            panic!("WorkspacePlugin dependency is missing");
        }
        app.world_mut().insert_resource(WorkspaceRegistry {
            agent_images_root: Arc::new(self.agent_images_root),
            ..WorkspaceRegistry::default()
        });
        app.add_system(&self.schedule, begin_workspace_command_system)
            .add_system(&self.schedule, route_agent_message_system)
            .add_system(&self.schedule, route_agent_turn_abort_system)
            .add_system(&self.schedule, route_mcl_command_system)
            .add_system(&self.schedule, collect_mcl_command_response_system)
            .add_system(&self.schedule, collect_agent_image_system)
            .add_system(&self.schedule, collect_agent_initialization_system);
    }
}

pub trait WorldWorkspaceExt {
    fn start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition);
    fn stop_workspace(&self, id: impl Into<String>, workspace: Entity);
    fn workspace_by_id(&self, id: &ResourceId) -> Option<Entity>;
    fn workspaces(&self) -> Vec<Entity>;
    fn workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity>;
    fn workspace_manager(&self, workspace: Entity) -> Option<Entity>;
    fn workspace_of(&self, agent: Entity) -> Option<Entity>;
}

impl WorldWorkspaceExt for World {
    fn start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition) {
        self.send_event(StartWorkspace {
            id: id.into(),
            definition,
        });
    }

    fn stop_workspace(&self, id: impl Into<String>, workspace: Entity) {
        self.send_event(StopWorkspace {
            id: id.into(),
            workspace,
        });
    }

    fn workspace_by_id(&self, id: &ResourceId) -> Option<Entity> {
        self.entity_by_resource_id(id).ok()
    }

    fn workspaces(&self) -> Vec<Entity> {
        self.get_resource::<WorkspaceRegistry>()
            .map(|r| {
                r.workspaces
                    .iter()
                    .copied()
                    .filter(|e| self.is_alive(*e))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity> {
        self.get_component::<Workspace>(workspace)?.agent(name)
    }

    fn workspace_manager(&self, workspace: Entity) -> Option<Entity> {
        self.get_component::<Workspace>(workspace)?.manager()
    }

    fn workspace_of(&self, agent: Entity) -> Option<Entity> {
        self.get_component::<agent_plugin::Agent>(agent)
            .map(|a| a.info.workspace_id)
    }
}
