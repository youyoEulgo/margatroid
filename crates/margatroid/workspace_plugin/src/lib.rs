use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Arc;

use agent_image_loader_plugin::{
    AgentImageDefaultVisibility, AgentImageIdentity, AgentImageModelConfig, AgentImageSoul,
    LoadAgentImage, LoadAgentImageResult,
};
use agent_plugin::{AgentCreateRequest, AgentCreated, AgentWorkspaceId};
use app_runtime_plugin::{RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::{AgentInferenceSnapshot, GlobalModelRoutes, WorldInferenceExt};
use margatroid_types::{
    AgentMessage, Message, MessageIntent, ResourceRef, RouteAgentMessage, WorkspaceAgentDefinition,
    WorkspaceDefinition,
};
use memory_plugin::{AgentMemory, WorldMemoryExt};
use tool_plugin::AgentToolEnvironment;

pub use margatroid_types::StartWorkspace;

const MAX_ERROR_MESSAGE_BYTES: usize = 512;
const MAX_LOGICAL_NAME_BYTES: usize = 128;

pub struct WorkspacePlugin {
    agent_images_root: PathBuf,
    schedule: String,
}

impl WorkspacePlugin {
    pub fn open(agent_images_root: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        Ok(Self {
            agent_images_root: normalize_agent_images_root(agent_images_root.into())?,
            schedule: RuntimePlugin::UPDATE.to_owned(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for WorkspacePlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("WorkspacePlugin requires RuntimePlugin");
        }
        if !app.world().contains_resource::<GlobalModelRoutes>() {
            panic!("WorkspacePlugin requires InferencePlugin");
        }
        if app.world().contains_resource::<WorkspaceRegistry>() {
            panic!("WorkspacePlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("WorkspacePlugin schedule does not exist");
        }

        let schedule = self.schedule;
        app.world_mut().insert_resource(WorkspaceRegistry {
            agent_images_root: Arc::new(self.agent_images_root),
            ready: HashMap::new(),
            pending: HashMap::new(),
            image_requests: HashMap::new(),
            agent_requests: HashMap::new(),
        });
        app.add_system(&schedule, begin_workspace_command_system)
            .add_system(&schedule, route_agent_message_system)
            .add_system(&schedule, collect_agent_image_system)
            .add_system(&schedule, collect_agent_created_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartWorkspaceResult {
    pub id: String,
    pub result: Result<Entity, WorkspaceError>,
}

impl Event for StartWorkspaceResult {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadWorkspace {
    pub id: String,
    pub workspace: Entity,
    pub definition: WorkspaceDefinition,
}

impl Event for ReloadWorkspace {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadWorkspaceResult {
    pub id: String,
    pub previous: Entity,
    pub result: Result<Entity, WorkspaceError>,
}

impl Event for ReloadWorkspaceResult {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopWorkspace {
    pub id: String,
    pub workspace: Entity,
}

impl Event for StopWorkspace {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopWorkspaceResult {
    pub id: String,
    pub workspace: Entity,
    pub result: Result<(), WorkspaceError>,
}

impl Event for StopWorkspaceResult {}

fn route_agent_message_system(world: &mut World) {
    let requests = world
        .event_reader::<RouteAgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let Some(workspace) =
            world.workspace(&request.workspace.project_root, &request.workspace.name)
        else {
            tracing::warn!(id = %request.id, "agent message workspace was not found");
            continue;
        };
        let Some(configuration) = world.get_component::<WorkspaceConfiguration>(workspace) else {
            tracing::warn!(id = %request.id, "agent message workspace configuration is missing");
            continue;
        };
        let agent_name = request
            .agent
            .unwrap_or_else(|| configuration.definition().manager.clone());
        let Some(agent) = world.workspace_agent(workspace, &agent_name) else {
            tracing::warn!(id = %request.id, agent = %agent_name, "agent message agent was not found");
            continue;
        };
        if !matches!(request.message, Message::User { .. }) {
            tracing::warn!(id = %request.id, "route agent message only accepts User messages");
            continue;
        }
        world.send_event(AgentMessage {
            id: request.id,
            agent,
            message: request.message,
            intent: MessageIntent::UserWithoutToolCalls,
        });
    }
}

pub struct WorkspaceIdentity {
    name: Arc<str>,
    project_root: Arc<PathBuf>,
}

impl WorkspaceIdentity {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn project_root(&self) -> &Path {
        self.project_root.as_path()
    }
}

impl Component for WorkspaceIdentity {}

pub struct WorkspaceConfiguration {
    definition: Arc<WorkspaceDefinition>,
}

impl WorkspaceConfiguration {
    pub fn definition(&self) -> &WorkspaceDefinition {
        &self.definition
    }
}

impl Component for WorkspaceConfiguration {}

pub struct WorkspaceAgents {
    manager: Entity,
    agents: BTreeMap<String, Entity>,
}

impl WorkspaceAgents {
    pub fn manager(&self) -> Entity {
        self.manager
    }

    pub fn agent(&self, name: &str) -> Option<Entity> {
        self.agents.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_ {
        self.agents
            .iter()
            .map(|(name, entity)| (name.as_str(), *entity))
    }
}

impl Component for WorkspaceAgents {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    kind: WorkspaceErrorKind,
    message: String,
}

impl WorkspaceError {
    fn new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_MESSAGE_BYTES {
            const SUFFIX: &str = "...";
            let mut boundary = MAX_ERROR_MESSAGE_BYTES - SUFFIX.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(SUFFIX);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WorkspaceError {}

pub trait WorldWorkspaceExt {
    fn start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition);
    fn reload_workspace(
        &self,
        id: impl Into<String>,
        workspace: Entity,
        definition: WorkspaceDefinition,
    );
    fn stop_workspace(&self, id: impl Into<String>, workspace: Entity);
    fn workspaces(&self) -> Vec<Entity>;
    fn workspace(&self, project_root: &Path, name: &str) -> Option<Entity>;
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

    fn reload_workspace(
        &self,
        id: impl Into<String>,
        workspace: Entity,
        definition: WorkspaceDefinition,
    ) {
        self.send_event(ReloadWorkspace {
            id: id.into(),
            workspace,
            definition,
        });
    }

    fn stop_workspace(&self, id: impl Into<String>, workspace: Entity) {
        self.send_event(StopWorkspace {
            id: id.into(),
            workspace,
        });
    }

    fn workspaces(&self) -> Vec<Entity> {
        self.get_resource::<WorkspaceRegistry>()
            .map(|registry| {
                registry
                    .ready
                    .values()
                    .copied()
                    .filter(|workspace| is_registered_workspace(self, *workspace))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn workspace(&self, project_root: &Path, name: &str) -> Option<Entity> {
        let project_root = normalize_project_root(project_root.to_path_buf()).ok()?;
        let key = WorkspaceKey {
            project_root,
            name: name.to_owned(),
        };
        let workspace = self
            .get_resource::<WorkspaceRegistry>()?
            .ready
            .get(&key)
            .copied()?;
        is_registered_workspace(self, workspace).then_some(workspace)
    }

    fn workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity> {
        if !is_registered_workspace(self, workspace) {
            return None;
        }
        let agent = self
            .get_component::<WorkspaceAgents>(workspace)?
            .agent(name)?;
        self.is_alive(agent).then_some(agent)
    }

    fn workspace_manager(&self, workspace: Entity) -> Option<Entity> {
        if !is_registered_workspace(self, workspace) {
            return None;
        }
        let manager = self.get_component::<WorkspaceAgents>(workspace)?.manager();
        self.is_alive(manager).then_some(manager)
    }

    fn workspace_of(&self, agent: Entity) -> Option<Entity> {
        let workspace = self
            .get_component::<AgentWorkspaceId>(agent)?
            .workspace_id();
        is_registered_workspace(self, workspace).then_some(workspace)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkspaceKey {
    project_root: PathBuf,
    name: String,
}

#[derive(Clone, Copy)]
enum PendingWorkspaceKind {
    Start,
    Reload { previous: Entity },
}

struct PendingWorkspace {
    kind: PendingWorkspaceKind,
    workspace: Entity,
    definition: WorkspaceDefinition,
    images: BTreeMap<String, Result<Entity, WorkspaceError>>,
    prepared: BTreeMap<String, PreparedWorkspaceAgent>,
    agents: BTreeMap<String, Entity>,
}

struct PreparedWorkspaceAgent {
    name: String,
    agent_id: String,
    system_prompt: String,
    messages: Vec<Message>,
    default_visibility: BTreeSet<ResourceRef>,
    inference_snapshot: AgentInferenceSnapshot,
    tool_environment: AgentToolEnvironment,
    memory: AgentMemory,
}

struct WorkspaceRegistry {
    agent_images_root: Arc<PathBuf>,
    ready: HashMap<WorkspaceKey, Entity>,
    pending: HashMap<String, PendingWorkspace>,
    image_requests: HashMap<String, (String, String)>,
    agent_requests: HashMap<String, (String, String)>,
}

impl Resource for WorkspaceRegistry {}

fn begin_workspace_command_system(world: &mut World) {
    let starts = world
        .event_reader::<StartWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let reloads = world
        .event_reader::<ReloadWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let stops = world
        .event_reader::<StopWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    for event in starts {
        let id = event.id;
        if let Err(error) =
            begin_workspace_start(world, &id, event.definition, PendingWorkspaceKind::Start)
        {
            world.send_event(StartWorkspaceResult {
                id,
                result: Err(error),
            });
        }
    }

    for event in reloads {
        let id = event.id;
        let previous = event.workspace;
        if let Err(error) = begin_workspace_reload(world, &id, previous, event.definition) {
            world.send_event(ReloadWorkspaceResult {
                id,
                previous,
                result: Err(error),
            });
        }
    }

    for event in stops {
        let result = if event.id.is_empty() {
            Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "workspace request id cannot be empty",
            ))
        } else {
            stop_workspace_inner(world, event.workspace)
        };
        world.send_event(StopWorkspaceResult {
            id: event.id,
            workspace: event.workspace,
            result,
        });
    }
}

fn begin_workspace_reload(
    world: &mut World,
    id: &str,
    previous: Entity,
    definition: WorkspaceDefinition,
) -> Result<(), WorkspaceError> {
    validate_request_id(world, id)?;
    let definition = validate_definition(definition)?;
    let current_key = ready_workspace_key(world, previous)?;
    let next_key = WorkspaceKey::from_definition(&definition);
    if current_key != next_key {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceMismatch,
            "reload definition does not identify the current workspace",
        ));
    }
    if pending_contains_key(world, &next_key) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::DuplicateWorkspace,
            "workspace already has a pending operation",
        ));
    }
    stop_workspace_inner(world, previous)?;
    begin_workspace_start_validated(
        world,
        id,
        definition,
        PendingWorkspaceKind::Reload { previous },
    )
}

fn begin_workspace_start(
    world: &mut World,
    id: &str,
    definition: WorkspaceDefinition,
    kind: PendingWorkspaceKind,
) -> Result<(), WorkspaceError> {
    validate_request_id(world, id)?;
    let definition = validate_definition(definition)?;
    begin_workspace_start_validated(world, id, definition, kind)
}

fn begin_workspace_start_validated(
    world: &mut World,
    id: &str,
    definition: WorkspaceDefinition,
    kind: PendingWorkspaceKind,
) -> Result<(), WorkspaceError> {
    let key = WorkspaceKey::from_definition(&definition);
    let duplicate = world
        .get_resource::<WorkspaceRegistry>()
        .is_some_and(|registry| {
            registry.ready.contains_key(&key)
                || registry
                    .pending
                    .values()
                    .any(|pending| WorkspaceKey::from_definition(&pending.definition) == key)
        });
    if duplicate {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::DuplicateWorkspace,
            "workspace is already started or being started",
        ));
    }

    let workspace = world.spawn();
    assert!(world.insert_component(
        workspace,
        WorkspaceIdentity {
            name: Arc::from(definition.name.as_str()),
            project_root: Arc::new(definition.project_root.clone()),
        },
    ));
    assert!(world.insert_component(
        workspace,
        WorkspaceConfiguration {
            definition: Arc::new(definition.clone()),
        },
    ));
    if world
        .load_workspace_model_routes(workspace, &definition.project_root)
        .is_err()
    {
        world.despawn(workspace);
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InferenceSetupFailed,
            "workspace model routes could not be loaded",
        ));
    }

    let image_requests = definition
        .agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            (
                format!("{id}/image/{index}"),
                agent.name.clone(),
                agent.image.clone(),
            )
        })
        .collect::<Vec<_>>();
    let registry = world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed");
    registry.pending.insert(
        id.to_owned(),
        PendingWorkspace {
            kind,
            workspace,
            definition,
            images: BTreeMap::new(),
            prepared: BTreeMap::new(),
            agents: BTreeMap::new(),
        },
    );
    for (request_id, name, _) in &image_requests {
        registry
            .image_requests
            .insert(request_id.clone(), (id.to_owned(), name.clone()));
    }
    for (request_id, _, reference) in image_requests {
        world.send_event(LoadAgentImage {
            id: request_id,
            reference,
        });
    }
    Ok(())
}

fn collect_agent_image_system(world: &mut World) {
    let outcomes = world
        .event_reader::<LoadAgentImageResult>()
        .into_iter()
        .map(|event| (event.id.clone(), event.result.clone()))
        .collect::<Vec<_>>();

    for (child_id, result) in outcomes {
        let route = world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed")
            .image_requests
            .remove(&child_id);
        let Some((request_id, agent_name)) = route else {
            continue;
        };
        let mapped = result.map_err(|error| {
            WorkspaceError::new(
                WorkspaceErrorKind::AgentImageLoadFailed,
                format!("agent image loading failed with {:?}", error.kind()),
            )
        });
        let action = {
            let registry = world
                .get_resource_mut::<WorkspaceRegistry>()
                .expect("WorkspacePlugin is not installed");
            let Some(pending) = registry.pending.get_mut(&request_id) else {
                continue;
            };
            pending.images.insert(agent_name, mapped);
            if pending.images.len() != pending.definition.agents.len() {
                None
            } else if let Some(error) = pending
                .images
                .values()
                .find_map(|result| result.as_ref().err().cloned())
            {
                Some(Err(error))
            } else {
                Some(Ok(()))
            }
        };
        match action {
            Some(Err(error)) => fail_pending_workspace(world, &request_id, error),
            Some(Ok(())) => {
                if let Err(error) = prepare_workspace_agents(world, &request_id) {
                    fail_pending_workspace(world, &request_id, error);
                }
            }
            None => {}
        }
    }
}

fn prepare_workspace_agents(world: &mut World, request_id: &str) -> Result<(), WorkspaceError> {
    let (workspace, definition, images, agent_images_root) = {
        let registry = world
            .get_resource::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed");
        let pending = registry.pending.get(request_id).ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "pending workspace operation was not found",
            )
        })?;
        let images = pending
            .images
            .iter()
            .map(|(name, image)| {
                image
                    .as_ref()
                    .copied()
                    .map(|entity| (name.clone(), entity))
                    .map_err(Clone::clone)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        (
            pending.workspace,
            pending.definition.clone(),
            images,
            Arc::clone(&registry.agent_images_root),
        )
    };

    let mut prepared = BTreeMap::new();
    for (index, agent_definition) in definition.agents.iter().enumerate() {
        let image = images.get(&agent_definition.name).copied().ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::AgentImageComponentsMissing,
                "loaded AgentImage result was not found",
            )
        })?;
        let identity = world
            .get_component::<AgentImageIdentity>(image)
            .ok_or_else(agent_image_components_missing)?;
        if identity.reference() != &agent_definition.image {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceMismatch,
                "loaded AgentImage does not match the workspace definition",
            ));
        }
        let system_prompt = world
            .get_component::<AgentImageSoul>(image)
            .ok_or_else(agent_image_components_missing)?
            .as_str()
            .to_owned();
        let inference_snapshot = {
            let config = world
                .get_component::<AgentImageModelConfig>(image)
                .ok_or_else(agent_image_components_missing)?;
            world
                .build_agent_inference_snapshot(workspace, image, config)
                .map_err(|_| {
                    WorkspaceError::new(
                        WorkspaceErrorKind::InferenceSetupFailed,
                        "agent inference snapshot could not be built",
                    )
                })?
        };
        let mut default_visibility = world
            .get_component::<AgentImageDefaultVisibility>(image)
            .ok_or_else(agent_image_components_missing)?
            .resources()
            .cloned()
            .collect::<BTreeSet<_>>();
        default_visibility.extend(agent_definition.resources.iter().cloned());
        for disabled in &agent_definition.disable_resources {
            default_visibility.remove(disabled);
        }

        let image_root = image_root(&agent_images_root, &agent_definition.image);
        let tool_environment =
            AgentToolEnvironment::new(definition.project_root.clone(), image_root);
        let memory_path = agent_definition.memory_path.clone().unwrap_or_else(|| {
            default_memory_path(
                &definition.project_root,
                &definition.name,
                &agent_definition.name,
            )
        });
        let (memory, messages) = AgentMemory::open(memory_path).map_err(|error| {
            WorkspaceError::new(
                WorkspaceErrorKind::MemorySetupFailed,
                format!("agent memory could not be opened: {:?}", error.kind()),
            )
        })?;
        prepared.insert(
            agent_definition.name.clone(),
            PreparedWorkspaceAgent {
                name: agent_definition.name.clone(),
                agent_id: format!("{}.{}{}", definition.name, agent_definition.name, index),
                system_prompt,
                messages,
                default_visibility,
                inference_snapshot,
                tool_environment,
                memory,
            },
        );
    }

    let requests = definition
        .agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            let prepared = prepared
                .get(&agent.name)
                .expect("every workspace agent must have prepared material");
            (
                format!("{request_id}/agent/{index}"),
                prepared.name.clone(),
                AgentCreateRequest {
                    id: format!("{request_id}/agent/{index}"),
                    agent_id: prepared.agent_id.clone(),
                    workspace_id: workspace,
                    system_prompt: prepared.system_prompt.clone(),
                    messages: prepared.messages.clone(),
                    default_visibility: prepared.default_visibility.clone(),
                },
            )
        })
        .collect::<Vec<_>>();
    let registry = world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed");
    registry
        .pending
        .get_mut(request_id)
        .expect("pending workspace operation disappeared")
        .prepared = prepared;
    for (child_id, name, _) in &requests {
        registry
            .agent_requests
            .insert(child_id.clone(), (request_id.to_owned(), name.clone()));
    }
    for (_, _, request) in requests {
        world.send_event(request);
    }
    Ok(())
}

fn collect_agent_created_system(world: &mut World) {
    let events = world
        .event_reader::<AgentCreated>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut grouped = BTreeMap::<String, Vec<(String, AgentCreated)>>::new();
    for event in events {
        let route = world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed")
            .agent_requests
            .remove(&event.id);
        if let Some((request_id, name)) = route {
            grouped.entry(request_id).or_default().push((name, event));
        } else {
            cleanup_orphan_agent(world, event.agent);
        }
    }

    for (request_id, events) in grouped {
        let mut failure = None;
        for (name, event) in &events {
            if failure.is_some() {
                world.despawn(event.agent);
                continue;
            }
            if let Err(error) = attach_prepared_agent(world, &request_id, name, event.agent) {
                world.despawn(event.agent);
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            fail_pending_workspace(world, &request_id, error);
            continue;
        }
        let complete = world
            .get_resource::<WorkspaceRegistry>()
            .and_then(|registry| registry.pending.get(&request_id))
            .is_some_and(|pending| pending.agents.len() == pending.definition.agents.len());
        if complete {
            if let Err(error) = complete_pending_workspace(world, &request_id) {
                fail_pending_workspace(world, &request_id, error);
            }
        }
    }
}

fn attach_prepared_agent(
    world: &mut World,
    request_id: &str,
    name: &str,
    agent: Entity,
) -> Result<(), WorkspaceError> {
    let (workspace, prepared) = {
        let registry = world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed");
        let pending = registry.pending.get_mut(request_id).ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "pending workspace operation was not found",
            )
        })?;
        let prepared = pending.prepared.remove(name).ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceMismatch,
                "Agent creation result does not match prepared material",
            )
        })?;
        (pending.workspace, prepared)
    };
    if !world.is_alive(agent)
        || world
            .get_component::<AgentWorkspaceId>(agent)
            .map(AgentWorkspaceId::workspace_id)
            != Some(workspace)
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceMismatch,
            "created Agent does not belong to the pending workspace",
        ));
    }
    world
        .bind_agent_memory(agent, prepared.memory, &prepared.messages)
        .map_err(|error| {
            WorkspaceError::new(
                WorkspaceErrorKind::MemorySetupFailed,
                format!("agent memory could not be bound: {:?}", error.kind()),
            )
        })?;
    if !world.insert_component(agent, prepared.inference_snapshot)
        || !world.insert_component(agent, prepared.tool_environment)
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::ResourceSetupFailed,
            "created Agent could not receive its runtime components",
        ));
    }
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed")
        .pending
        .get_mut(request_id)
        .expect("pending workspace operation disappeared")
        .agents
        .insert(name.to_owned(), agent);
    Ok(())
}

fn complete_pending_workspace(world: &mut World, request_id: &str) -> Result<(), WorkspaceError> {
    let (workspace, manager, agents, key) = {
        let registry = world
            .get_resource::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed");
        let pending = registry.pending.get(request_id).ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "pending workspace operation was not found",
            )
        })?;
        let manager = pending
            .agents
            .get(&pending.definition.manager)
            .copied()
            .ok_or_else(|| {
                WorkspaceError::new(
                    WorkspaceErrorKind::InvalidDefinition,
                    "workspace manager Agent was not created",
                )
            })?;
        (
            pending.workspace,
            manager,
            pending.agents.clone(),
            WorkspaceKey::from_definition(&pending.definition),
        )
    };
    if !world.insert_component(workspace, WorkspaceAgents { manager, agents }) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "pending workspace entity is not alive",
        ));
    }
    let pending = {
        let registry = world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed");
        let pending = registry
            .pending
            .remove(request_id)
            .expect("pending workspace operation disappeared");
        registry.ready.insert(key, workspace);
        registry
            .image_requests
            .retain(|_, (parent, _)| parent != request_id);
        registry
            .agent_requests
            .retain(|_, (parent, _)| parent != request_id);
        pending
    };
    match pending.kind {
        PendingWorkspaceKind::Start => world.send_event(StartWorkspaceResult {
            id: request_id.to_owned(),
            result: Ok(workspace),
        }),
        PendingWorkspaceKind::Reload { previous } => world.send_event(ReloadWorkspaceResult {
            id: request_id.to_owned(),
            previous,
            result: Ok(workspace),
        }),
    }
    Ok(())
}

fn fail_pending_workspace(world: &mut World, request_id: &str, error: WorkspaceError) {
    let pending = {
        let registry = world
            .get_resource_mut::<WorkspaceRegistry>()
            .expect("WorkspacePlugin is not installed");
        let pending = registry.pending.remove(request_id);
        registry
            .image_requests
            .retain(|_, (parent, _)| parent != request_id);
        registry
            .agent_requests
            .retain(|_, (parent, _)| parent != request_id);
        pending
    };
    let Some(pending) = pending else {
        return;
    };
    for agent in pending.agents.values() {
        world.despawn(*agent);
    }
    world.despawn(pending.workspace);
    match pending.kind {
        PendingWorkspaceKind::Start => world.send_event(StartWorkspaceResult {
            id: request_id.to_owned(),
            result: Err(error),
        }),
        PendingWorkspaceKind::Reload { previous } => world.send_event(ReloadWorkspaceResult {
            id: request_id.to_owned(),
            previous,
            result: Err(error),
        }),
    }
}

fn stop_workspace_inner(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError> {
    let key = ready_workspace_key(world, workspace)?;
    let agents = world
        .get_component::<WorkspaceAgents>(workspace)
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceNotReady,
                "workspace does not have a complete Agent index",
            )
        })?
        .iter()
        .map(|(_, agent)| agent)
        .collect::<Vec<_>>();
    world
        .get_resource_mut::<WorkspaceRegistry>()
        .expect("WorkspacePlugin is not installed")
        .ready
        .remove(&key);
    for agent in agents {
        world.despawn(agent);
    }
    world.despawn(workspace);
    Ok(())
}

fn validate_request_id(world: &World, id: &str) -> Result<(), WorkspaceError> {
    if id.is_empty() {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InvalidRequest,
            "workspace request id cannot be empty",
        ));
    }
    if world
        .get_resource::<WorkspaceRegistry>()
        .is_some_and(|registry| registry.pending.contains_key(id))
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InvalidRequest,
            "workspace request id is already pending",
        ));
    }
    Ok(())
}

fn validate_definition(
    mut definition: WorkspaceDefinition,
) -> Result<WorkspaceDefinition, WorkspaceError> {
    validate_logical_name(&definition.name)?;
    definition.project_root = normalize_project_root(definition.project_root)?;
    if definition.agents.is_empty() {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InvalidDefinition,
            "workspace must define at least one Agent",
        ));
    }
    let mut names = HashSet::new();
    for agent in &mut definition.agents {
        validate_agent_definition(agent)?;
        if !names.insert(agent.name.clone()) {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidDefinition,
                "workspace Agent names must be unique",
            ));
        }
        if let Some(path) = agent.memory_path.take() {
            agent.memory_path = Some(normalize_memory_path(path)?);
        }
    }
    if !names.contains(&definition.manager) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InvalidDefinition,
            "workspace manager must name one configured Agent",
        ));
    }
    Ok(definition)
}

fn validate_agent_definition(agent: &WorkspaceAgentDefinition) -> Result<(), WorkspaceError> {
    validate_logical_name(&agent.name)
}

fn validate_logical_name(value: &str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.len() > MAX_LOGICAL_NAME_BYTES
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::InvalidDefinition,
            "workspace and Agent names must be safe single path segments",
        ));
    }
    Ok(())
}

fn normalize_agent_images_root(path: PathBuf) -> Result<PathBuf, WorkspaceError> {
    normalize_absolute_path(path).ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidAgentImagesRoot,
            "AgentImage root must be an absolute path without parent traversal",
        )
    })
}

fn normalize_project_root(path: PathBuf) -> Result<PathBuf, WorkspaceError> {
    normalize_absolute_path(path).ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidProjectRoot,
            "project root must be an absolute path without parent traversal",
        )
    })
}

fn normalize_memory_path(path: PathBuf) -> Result<PathBuf, WorkspaceError> {
    normalize_absolute_path(path).ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorKind::InvalidDefinition,
            "Agent memory path must be absolute and cannot contain parent traversal",
        )
    })
}

fn normalize_absolute_path(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == PathComponent::ParentDir)
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::ParentDir => unreachable!("parent components were rejected"),
            component => normalized.push(component.as_os_str()),
        }
    }
    Some(normalized)
}

fn image_root(root: &Path, reference: &margatroid_types::AgentImageReference) -> PathBuf {
    root.join(reference.scope())
        .join(reference.name())
        .join(reference.tag())
}

fn default_memory_path(project_root: &Path, workspace: &str, agent: &str) -> PathBuf {
    project_root
        .join(".margatroid")
        .join("workspaces")
        .join(workspace)
        .join("memory")
        .join(agent)
        .join("memory.sql")
}

fn agent_image_components_missing() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::AgentImageComponentsMissing,
        "loaded AgentImage is missing a required component",
    )
}

fn ready_workspace_key(world: &World, workspace: Entity) -> Result<WorkspaceKey, WorkspaceError> {
    if !world.is_alive(workspace) {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotAlive,
            "workspace entity is not alive",
        ));
    }
    let identity = world
        .get_component::<WorkspaceIdentity>(workspace)
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceMismatch,
                "workspace entity does not have WorkspaceIdentity",
            )
        })?;
    if world.get_component::<WorkspaceAgents>(workspace).is_none() {
        return Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotReady,
            "workspace does not have a complete Agent index",
        ));
    }
    let key = WorkspaceKey {
        project_root: identity.project_root().to_path_buf(),
        name: identity.name().to_owned(),
    };
    match world
        .get_resource::<WorkspaceRegistry>()
        .and_then(|registry| registry.ready.get(&key))
    {
        Some(registered) if *registered == workspace => Ok(key),
        Some(_) => Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceMismatch,
            "workspace registry points to a different entity",
        )),
        None => Err(WorkspaceError::new(
            WorkspaceErrorKind::WorkspaceNotReady,
            "workspace is not registered for requests",
        )),
    }
}

fn is_registered_workspace(world: &World, workspace: Entity) -> bool {
    ready_workspace_key(world, workspace).is_ok()
}

fn pending_contains_key(world: &World, key: &WorkspaceKey) -> bool {
    world
        .get_resource::<WorkspaceRegistry>()
        .is_some_and(|registry| {
            registry
                .pending
                .values()
                .any(|pending| WorkspaceKey::from_definition(&pending.definition) == *key)
        })
}

fn cleanup_orphan_agent(world: &mut World, agent: Entity) {
    let parent_is_dead = world
        .get_component::<AgentWorkspaceId>(agent)
        .is_some_and(|workspace| !world.is_alive(workspace.workspace_id()));
    if parent_is_dead {
        world.despawn(agent);
    }
}

impl WorkspaceKey {
    fn from_definition(definition: &WorkspaceDefinition) -> Self {
        Self {
            project_root: definition.project_root.clone(),
            name: definition.name.clone(),
        }
    }
}
