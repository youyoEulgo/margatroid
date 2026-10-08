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
use margatroid_types::{ResourceRef, StartWorkspace, WorkspaceDefinition};
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
    collect_workspace_stop_system, route_agent_turn_abort_system, route_mcl_command_system,
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

#[cfg(test)]
mod stop_tests {
    use super::*;
    use agent_plugin::*;
    use core_plugin::World;
    use lua_runtime_plugin::{LuaRuntimeTaskFinished, LuaVmId, LuaVmOwner, LuaVmState};

    fn agent(world: &mut World, workspace: Entity, creating: bool) -> Entity {
        let entity = world.spawn();
        let (sender, _) = tokio::sync::oneshot::channel();
        let model = AgentModelInfo {
            provider: "test".into(),
            model: "test".into(),
            context_window_tokens: 100,
        };
        world.insert_component(
            entity,
            Agent {
                info: AgentInfo {
                    workspace_id: workspace,
                    image_entity: workspace,
                    model: model.clone(),
                    project_root: Default::default(),
                    image_root: Default::default(),
                    home_root: Default::default(),
                    image_dependencies: Default::default(),
                },
                creation: AgentCreationState {
                    request_id: "creation".into(),
                    reply: AgentCreateReply::new(sender),
                    initialization: AgentInitializationState {
                        complete: !creating,
                        failed: None,
                    },
                },
                mcl: Default::default(),
                resources: Default::default(),
                memory: AgentMemoryHandle::new(Arc::new(
                    memory_plugin::AgentMemory::open(":memory:").unwrap(),
                )),
                inference: AgentInferenceState {
                    model,
                    pending: Default::default(),
                },
                tools: Default::default(),
                lua: AgentLuaState {
                    request_id: creating.then(|| "creation".into()),
                    vm_id: (!creating).then_some(LuaVmId(99)),
                },
                lifecycle: if creating {
                    AgentLifecycleState::Creating
                } else {
                    AgentLifecycleState::Running
                },
                turn: Default::default(),
                token_usage: Default::default(),
                last_error: None,
            },
        );
        entity
    }

    fn finish(world: &mut World, entity: Entity, creating: bool) {
        handle_lua_vm_finished(
            world,
            LuaRuntimeTaskFinished {
                request_id: "creation".into(),
                vm_id: (!creating).then_some(LuaVmId(99)),
                owner: LuaVmOwner {
                    owner_id: "test".into(),
                },
                state: LuaVmState::Cancelled,
                error: None,
            },
        );
        assert!(
            world.is_alive(entity),
            "VM completion must not bypass owner release"
        );
    }

    #[test]
    fn stop_retains_vm_and_resource_until_finished_including_unlisted_creation() {
        for creating in [false, true] {
            let mut app = App::new();
            app.add_plugin(RuntimePlugin::default())
                .add_plugin(resource_id_plugin::ResourceIdPlugin);
            let world = app.world_mut();
            world.insert_resource(WorkspaceRegistry::default());
            let workspace = world.spawn();
            let entity = agent(world, workspace, creating);
            let resource = resource_library_plugin::install_resource(
                world,
                ResourceRef::parse("prompt:system/stop-test").unwrap(),
                entity,
                resource_library_plugin::ResourceDefinition::Prompt(
                    resource_library_plugin::PromptResource {
                        role: "system".into(),
                        content: Arc::from("test"),
                    },
                ),
            )
            .unwrap();
            handler::stop_workspace_entity(world, workspace).unwrap();
            system::collect_workspace_stop_system(world);
            assert!(world.is_alive(entity));
            assert!(world.is_alive(resource));
            assert!(world.is_alive(workspace));
            world.advance_event_frame();
            assert_eq!(world.event_reader::<AgentControl>().len(), 1);
            world.get_component_mut::<Agent>(entity).unwrap().lifecycle =
                AgentLifecycleState::Stopping;
            finish(world, entity, creating);
            system::collect_workspace_stop_system(world);
            assert!(!world.is_alive(entity));
            assert!(!world.is_alive(resource));
            assert!(!world.is_alive(workspace));
        }
    }

    #[test]
    fn stop_success_is_emitted_only_after_vm_completion() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default());
        let world = app.world_mut();
        world.insert_resource(WorkspaceRegistry::default());
        let workspace = world.spawn();
        let entity = agent(world, workspace, false);
        world.send_event(StopWorkspace {
            id: "stop".into(),
            workspace,
        });
        world.advance_event_frame();
        system::begin_workspace_command_system(world);
        world.advance_event_frame();
        assert!(world.event_reader::<StopWorkspaceResult>().is_empty());
        system::collect_workspace_stop_system(world);
        world.advance_event_frame();
        assert!(world.event_reader::<StopWorkspaceResult>().is_empty());
        finish(world, entity, false);
        system::collect_workspace_stop_system(world);
        world.advance_event_frame();
        let results = world.event_reader::<StopWorkspaceResult>();
        assert_eq!(results.len(), 1);
        assert!(results.into_iter().next().unwrap().result.is_ok());
    }

    #[test]
    fn finished_vm_waits_for_inflight_tool_and_inference() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default());
        let world = app.world_mut();
        world.insert_resource(WorkspaceRegistry::default());
        let workspace = world.spawn();
        let entity = agent(world, workspace, false);
        handler::stop_workspace_entity(world, workspace).unwrap();
        finish(world, entity, false);
        let tool_key = (entity, "turn".to_owned(), "call".to_owned());
        let inference_key = (entity, "infer".to_owned());
        let agent = world.get_component_mut::<Agent>(entity).unwrap();
        agent.tools.pending.insert(
            tool_key.clone(),
            AgentToolPending {
                turn_id: "turn".into(),
                tool_call_id: "call".into(),
                resource_id: ResourceRef::parse("tool:local/test").unwrap(),
                executor: agent_plugin::ToolExecutor::SkillLoader,
            },
        );
        agent.inference.pending.insert(
            inference_key.clone(),
            AgentInferencePending {
                id: "infer".into(),
                tool_schema: vec![],
            },
        );
        system::collect_workspace_stop_system(world);
        assert!(world.is_alive(entity));
        world
            .get_component_mut::<Agent>(entity)
            .unwrap()
            .tools
            .pending
            .remove(&tool_key);
        system::collect_workspace_stop_system(world);
        assert!(world.is_alive(entity));
        world
            .get_component_mut::<Agent>(entity)
            .unwrap()
            .inference
            .pending
            .remove(&inference_key);
        system::collect_workspace_stop_system(world);
        assert!(!world.is_alive(entity));
        assert!(!world.is_alive(workspace));
    }

    #[test]
    fn queued_creation_prevents_workspace_reclamation() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default());
        let world = app.world_mut();
        world.insert_resource(WorkspaceRegistry::default());
        let workspace = world.spawn();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .unwrap()
            .pending_agents
            .insert("queued".into(), (workspace, "agent".into(), receiver));
        handler::stop_workspace_entity(world, workspace).unwrap();
        system::collect_workspace_stop_system(world);
        assert!(world.is_alive(workspace));
        drop(sender);
        system::collect_agent_image_system(world);
        system::collect_workspace_stop_system(world);
        assert!(!world.is_alive(workspace));
    }
}

impl Component for Workspace {}
impl library_component_plugin::LibraryComponent for Workspace {}

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
            .add_system(&self.schedule, route_agent_turn_abort_system)
            .add_system(&self.schedule, route_mcl_command_system)
            .add_system(&self.schedule, collect_mcl_command_response_system)
            .add_system(&self.schedule, collect_agent_image_system)
            .add_system(&self.schedule, collect_agent_initialization_system)
            .add_system(&self.schedule, collect_workspace_stop_system);
    }
}

pub trait WorldWorkspaceExt {
    fn start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition);
    fn stop_workspace(&self, id: impl Into<String>, workspace: Entity);
    fn workspace_by_id(
        &self,
        id: &ResourceRef,
    ) -> Result<Entity, resource_id_plugin::ResourceIdLookupError>;
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

    fn workspace_by_id(
        &self,
        id: &ResourceRef,
    ) -> Result<Entity, resource_id_plugin::ResourceIdLookupError> {
        self.entity_by_resource_id(id)
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
