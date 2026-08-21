use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_image_loader_plugin::AgentImageLoaderPluginInstalled;
use agent_image_loader_plugin::{AgentImage, LoadAgentImage, LoadAgentImageResult};
use agent_plugin::AgentPluginInstalled;
use agent_plugin::{
    Agent, AgentControl, AgentControlKind, AgentControlReply, AgentCreateReply, AgentCreateRequest,
    AgentInitializationCompleted, AgentMemoryHandle, AgentMessage, AgentModelInfo,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::WorkspaceModelRoutesRegistry;
use lua_runtime_plugin::LuaRuntimePluginInstalled;
use margatroid_types::{
    ResourceId, RouteAgentMessage, RouteAgentTurnAbort, RouteMclCommand, StartWorkspace,
    WorkspaceDefinition, WorkspaceReference,
};
use mcl_plugin::{MclCommandId, MclCommandReply, MclCommandRequest, MclPluginInstalled};
use memory_plugin::AgentMemory;
use memory_plugin::MemoryPluginInstalled;
use resource_id_plugin::ResourceIdPluginInstalled;
use resource_id_plugin::WorldResourceIdExt;
use tool_plugin::ToolPluginInstalled;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    InvalidRequest,
    InvalidDefinition,
    InvalidProjectRoot,
    InvalidAgentImagesRoot,
    DuplicateWorkspace,
    WorkspaceNotAlive,
    WorkspaceNotReady,
    WorkspaceMismatch,
    AgentImageLoadFailed,
    AgentImageComponentsMissing,
    InferenceSetupFailed,
    MemorySetupFailed,
    ResourceSetupFailed,
    AgentCreateFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    kind: WorkspaceErrorKind,
    message: String,
}
impl WorkspaceError {
    pub fn new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind.clone()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for WorkspaceError {}

#[derive(Clone, Debug)]
pub struct StartWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for StartWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct ReloadWorkspace {
    pub id: String,
    pub workspace: Entity,
    pub definition: WorkspaceDefinition,
}
impl Event for ReloadWorkspace {}
#[derive(Clone, Debug)]
pub struct ReloadWorkspaceResult {
    pub id: String,
    pub previous: Entity,
    pub result: Result<Entity, WorkspaceError>,
}
impl Event for ReloadWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
}
impl Event for StopWorkspace {}
#[derive(Clone, Debug)]
pub struct StopWorkspaceResult {
    pub id: String,
    pub workspace: Entity,
    pub result: Result<(), WorkspaceError>,
}
impl Event for StopWorkspaceResult {}
#[derive(Clone, Debug)]
pub struct StopWorkspaceByReference {
    pub id: String,
    pub workspace: WorkspaceReference,
}
impl Event for StopWorkspaceByReference {}
#[derive(Clone, Debug)]
pub struct StopWorkspaceByReferenceResult {
    pub id: String,
    pub workspace: WorkspaceReference,
    pub result: Result<(), WorkspaceError>,
}
impl Event for StopWorkspaceByReferenceResult {}

#[derive(Clone, Debug)]
pub struct Workspace {
    definition: Arc<WorkspaceDefinition>,
    project_root: Arc<PathBuf>,
    manager_name: String,
    agents: BTreeMap<String, Entity>,
    states: BTreeMap<String, WorkspaceAgentState>,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAgentState {
    Creating,
    Ready { agent: Entity },
    Failed { error: WorkspaceError },
}
#[derive(Default)]
pub struct WorkspaceRegistry {
    pub agent_images_root: Arc<PathBuf>,
    pub workspaces: Vec<Entity>,
    pub pending_images: BTreeMap<String, (Entity, String)>,
    pub pending_agents: BTreeMap<
        String,
        (
            Entity,
            String,
            tokio::sync::oneshot::Receiver<Result<Entity, margatroid_types::AgentError>>,
        ),
    >,
    pub pending_mcl_commands: BTreeMap<
        String,
        (
            std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
            tokio::sync::oneshot::Receiver<
                Result<mcl_plugin::MclCommandValue, mcl_plugin::MclError>,
            >,
        ),
    >,
}
impl Resource for WorkspaceRegistry {}

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
        self.get_component::<Agent>(agent)
            .map(|a| a.info.workspace_id)
    }
}

fn begin_workspace_command_system(world: &mut World) {
    let starts = world
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in starts {
        let result = create_workspace(world, &request.definition);
        world.send_event(StartWorkspaceResult {
            id: request.id,
            result,
        });
    }
    let stops = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in stops {
        let result = stop_workspace_entity(world, request.workspace);
        world.send_event(StopWorkspaceResult {
            id: request.id,
            workspace: request.workspace,
            result,
        });
    }

    let references = world
        .event_reader::<StopWorkspaceByReference>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in references {
        let result = world
            .entity_by_resource_id(&request.workspace.id)
            .map_err(|_| {
                WorkspaceError::new(
                    WorkspaceErrorKind::WorkspaceNotAlive,
                    "workspace is not alive",
                )
            })
            .and_then(|workspace| {
                let matches_root = world
                    .get_component::<Workspace>(workspace)
                    .is_some_and(|value| value.project_root() == request.workspace.project_root);
                if !matches_root {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorKind::WorkspaceMismatch,
                        "workspace reference does not match the active project",
                    ));
                }
                stop_workspace_entity(world, workspace)
            });
        world.send_event(StopWorkspaceByReferenceResult {
            id: request.id,
            workspace: request.workspace,
            result,
        });
    }

    let reloads = world
        .event_reader::<ReloadWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in reloads {
        let result = stop_workspace_entity(world, request.workspace)
            .and_then(|_| create_workspace(world, &request.definition));
        world.send_event(ReloadWorkspaceResult {
            id: request.id,
            previous: request.workspace,
            result,
        });
    }
}

fn stop_workspace_entity(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError> {
    if !world.is_alive(workspace) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "workspace is not alive",
        ));
    }
    let agents = world
        .get_component::<Workspace>(workspace)
        .map(|value| value.iter().map(|(_, entity)| entity).collect::<Vec<_>>())
        .unwrap_or_default();
    for agent in agents {
        let (sender, _) = tokio::sync::oneshot::channel();
        world.send_event(AgentControl {
            id: format!("stop-{:?}", agent),
            agent,
            control: AgentControlKind::Stop,
            reply: AgentControlReply::new(sender),
        });
        if world.is_alive(agent) {
            world.despawn(agent);
        }
    }
    if let Some(registry) = world.get_resource_mut::<WorkspaceRegistry>() {
        registry.workspaces.retain(|entity| *entity != workspace);
        registry
            .pending_images
            .retain(|_, (owner, _)| *owner != workspace);
        registry
            .pending_agents
            .retain(|_, (owner, _, _)| *owner != workspace);
    }
    if let Some(routes) = world.get_resource_mut::<WorkspaceModelRoutesRegistry>() {
        routes.remove(workspace);
    }
    world.despawn(workspace);
    Ok(())
}
fn create_workspace(
    world: &mut World,
    definition: &WorkspaceDefinition,
) -> Result<Entity, WorkspaceError> {
    if world.entity_by_resource_id(&definition.id).is_ok() {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::DuplicateWorkspace,
            "workspace already exists",
        ));
    }
    let entity = world.spawn();
    world.insert_component(entity, definition.id.clone());
    let mut workspace = Workspace {
        definition: Arc::new(definition.clone()),
        project_root: Arc::new(definition.project_root.clone()),
        manager_name: definition.manager.clone(),
        agents: BTreeMap::new(),
        states: BTreeMap::new(),
    };
    for agent in &definition.agents {
        workspace
            .states
            .insert(agent.name.clone(), WorkspaceAgentState::Creating);
        let image_request_id = format!("workspace-agent-{:?}-{}", entity, agent.name);
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed")
            .pending_images
            .insert(image_request_id.clone(), (entity, agent.name.clone()));
        world.send_event(LoadAgentImage {
            id: image_request_id,
            reference: agent.image.clone(),
        });
    }
    world.insert_component(entity, workspace);
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed")
        .workspaces
        .push(entity);
    Ok(entity)
}
fn route_agent_message_system(world: &mut World) {
    let events = world
        .event_reader::<RouteAgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let workspace = world.entity_by_resource_id(&event.workspace.id).ok();
        let agent = event
            .agent
            .as_ref()
            .and_then(|id| world.entity_by_resource_id(id).ok())
            .or_else(|| workspace.and_then(|id| world.workspace_manager(id)));
        if let Some(agent) = agent {
            world.send_event(AgentMessage {
                id: event.id,
                agent,
                message: event.message,
                usage: None,
            });
        }
    }
}
fn route_agent_turn_abort_system(world: &mut World) {
    let events = world
        .event_reader::<RouteAgentTurnAbort>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let workspace = world.entity_by_resource_id(&event.workspace.id).ok();
        let agent = event
            .agent
            .as_ref()
            .and_then(|id| world.entity_by_resource_id(id).ok())
            .or_else(|| workspace.and_then(|id| world.workspace_manager(id)));
        if let Some(agent) = agent {
            let (sender, _) = tokio::sync::oneshot::channel();
            world.send_event(AgentControl {
                id: event.id,
                agent,
                control: AgentControlKind::AbortTurn,
                reply: AgentControlReply::new(sender),
            });
        }
    }
}
fn route_mcl_command_system(world: &mut World) {
    let commands = world
        .event_reader::<RouteMclCommand>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for command in commands {
        if let Err(error) = route_mcl_command(world, command.clone()) {
            let _ = command.reply.send(Err(error.to_string()));
        }
    }
}

fn route_mcl_command(world: &mut World, command: RouteMclCommand) -> Result<(), WorkspaceError> {
    let workspace = world
        .entity_by_resource_id(&command.workspace.id)
        .map_err(|_| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotAlive,
                "workspace is not alive",
            )
        })?;
    let workspace_data = world.get_component::<Workspace>(workspace).ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "workspace component is missing",
        )
    })?;
    let agent = match command.agent.clone() {
        Some(agent_id) => world.entity_by_resource_id(&agent_id).map_err(|_| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "target agent is not alive",
            )
        })?,
        None => workspace_data.manager().ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "workspace manager is missing",
            )
        })?,
    };
    let agent_id = world
        .get_component::<ResourceId>(agent)
        .cloned()
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "agent identity is missing",
            )
        })?;
    let command_id = MclCommandId::new(command.id.clone()).map_err(|_| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidRequest,
            "MCL command id is invalid",
        )
    })?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    world.send_event(MclCommandRequest {
        id: command_id,
        agent_id,
        command: command.command,
        binding: command.binding,
        reply: MclCommandReply::new(sender),
    });
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotAlive,
                "WorkspacePlugin is not installed",
            )
        })?
        .pending_mcl_commands
        .insert(command.id, (command.reply, receiver));
    Ok(())
}

fn collect_mcl_command_response_system(world: &mut World) {
    let pending = world
        .get_resource_mut::<WorkspaceRegistry>()
        .map(|registry| std::mem::take(&mut registry.pending_mcl_commands))
        .unwrap_or_default();
    for (id, (reply, mut receiver)) in pending {
        match receiver.try_recv() {
            Ok(result) => {
                let result = result.map_err(|error| error.to_string()).and_then(|value| {
                    mcl_plugin::command_value_to_json(value).map_err(|error| error.to_string())
                });
                let _ = reply.send(result);
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                world
                    .get_resource_mut::<WorkspaceRegistry>()
                    .map(|registry| {
                        registry.pending_mcl_commands.insert(id, (reply, receiver));
                    });
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                let _ = reply.send(Err("MCL command reply was closed".to_owned()));
            }
        }
    }
}

fn collect_agent_image_system(world: &mut World) {
    let events = world
        .event_reader::<LoadAgentImageResult>()
        .into_iter()
        .map(|event| {
            (
                event.id.clone(),
                event.reference.clone(),
                event.result.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (event_id, reference, image_result) in events {
        let route = world
            .get_resource_mut::<WorkspaceRegistry>()
            .and_then(|registry| registry.pending_images.remove(&event_id));
        let Some((workspace, name)) = route else {
            continue;
        };
        let image = match image_result {
            Ok(image) => image,
            Err(error) => {
                tracing::error!(agent = %name, error = %error, "agent image load failed");
                if let Some(value) = world.get_component_mut::<Workspace>(workspace) {
                    value.states.insert(
                        name,
                        WorkspaceAgentState::Failed {
                            error: WorkspaceError::new(
                                WorkspaceErrorKind::AgentImageLoadFailed,
                                error.to_string(),
                            ),
                        },
                    );
                }
                continue;
            }
        };
        let Some(configuration) = world.get_component::<Workspace>(workspace).cloned() else {
            continue;
        };
        let Some(definition) = configuration
            .definition()
            .agents
            .iter()
            .find(|agent| agent.name == name && agent.image == reference)
            .cloned()
        else {
            continue;
        };
        let Some(image_data) = world.get_component::<AgentImage>(image).cloned() else {
            continue;
        };
        let base_lua = image_data.base_driver().program().clone();
        let model = AgentModelInfo {
            provider: "openai-compatible".into(),
            model: image_data.model().model().into(),
            context_window_tokens: 1_000_000,
        };
        let memory_path = definition.memory_path.clone().unwrap_or_else(|| {
            configuration
                .definition()
                .project_root
                .join(".margatroid")
                .join("workspaces")
                .join(&configuration.definition().name)
                .join("memory")
                .join(&definition.name)
                .join("memory.sql")
        });
        let Ok((memory, context)) = AgentMemory::open(memory_path) else {
            tracing::error!(agent = %name, "agent memory could not be opened");
            if let Some(value) = world.get_component_mut::<Workspace>(workspace) {
                value.states.insert(
                    name,
                    WorkspaceAgentState::Failed {
                        error: WorkspaceError::new(
                            WorkspaceErrorKind::MemorySetupFailed,
                            "agent memory could not be opened",
                        ),
                    },
                );
            }
            continue;
        };
        let request_id = format!("create-{}", event_id);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        world
            .get_resource_mut::<WorkspaceRegistry>()
            .map(|registry| {
                registry.pending_agents.insert(
                    request_id.clone(),
                    (workspace, definition.name.clone(), receiver),
                );
            });
        world.send_event(AgentCreateRequest {
            id: request_id,
            agent_id: definition.id,
            workspace_id: workspace,
            image_entity: image,
            base_lua: lua_runtime_plugin::LuaProgram {
                source: base_lua.source().to_owned(),
                origin: base_lua.origin().display().to_string(),
                entry: None,
                libraries: lua_runtime_plugin::LuaStandardLibraries::Safe,
            },
            project_root: configuration.project_root().to_path_buf(),
            image_root: world
                .get_resource::<WorkspaceRegistry>()
                .map(|registry| {
                    registry
                        .agent_images_root
                        .join(definition.image.scope())
                        .join(definition.image.name())
                        .join(definition.image.tag())
                })
                .unwrap_or_default(),
            home_root: configuration.definition().project_root.clone(),
            model,
            memory: AgentMemoryHandle::new(Arc::new(memory)),
            token_usage: context.token_usage,
            image_dependencies: Arc::from(
                image_data
                    .dependencies()
                    .iter()
                    .map(|dependency| dependency.resource_id().clone())
                    .collect::<Vec<_>>(),
            ),
            image_sources: image_data
                .dependencies()
                .iter()
                .filter_map(|dependency| {
                    dependency
                        .source()
                        .map(|source| (dependency.resource_id().clone(), Arc::<str>::from(source)))
                })
                .collect::<HashMap<_, _>>(),
            reply: AgentCreateReply::new(sender),
        });
    }
    let pending = world
        .get_resource_mut::<WorkspaceRegistry>()
        .map(|registry| std::mem::take(&mut registry.pending_agents))
        .unwrap_or_default();
    for (id, (workspace, name, mut receiver)) in pending {
        match receiver.try_recv() {
            Ok(Ok(agent)) => {
                tracing::info!(agent = %name, "agent created");
                if let Some(index) = world.get_component_mut::<Workspace>(workspace) {
                    index.agents.insert(name.clone(), agent);
                    index
                        .states
                        .insert(name, WorkspaceAgentState::Ready { agent });
                }
            }
            Ok(Err(error)) => {
                tracing::error!(agent = %name, error = %error, "agent create failed");
                if let Some(index) = world.get_component_mut::<Workspace>(workspace) {
                    index.states.insert(
                        name,
                        WorkspaceAgentState::Failed {
                            error: WorkspaceError::new(
                                WorkspaceErrorKind::AgentCreateFailed,
                                error.to_string(),
                            ),
                        },
                    );
                }
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                world
                    .get_resource_mut::<WorkspaceRegistry>()
                    .map(|registry| {
                        registry
                            .pending_agents
                            .insert(id, (workspace, name, receiver));
                    });
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }
}

fn collect_agent_initialization_system(world: &mut World) {
    let events = world
        .event_reader::<AgentInitializationCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for event in events {
        let Some(agent) = world.get_component::<Agent>(event.agent).cloned() else {
            continue;
        };
        let workspace = agent.info.workspace_id;
        let Some(agent_id) = world.get_component::<ResourceId>(event.agent).cloned() else {
            continue;
        };
        let Some(value) = world.get_component_mut::<Workspace>(workspace) else {
            continue;
        };
        let agent_name = value
            .definition()
            .agents
            .iter()
            .find(|definition| definition.id == agent_id)
            .map(|definition| definition.name.clone());
        if let Some(agent_name) = agent_name {
            tracing::info!(agent = %agent_name, "agent initialization completed");
            value.agents.insert(agent_name.clone(), event.agent);
            value.states.insert(
                agent_name,
                WorkspaceAgentState::Ready { agent: event.agent },
            );
        }
    }
}
