use crate::agent_events::StartAgent;
use crate::workspace_events::{StartWorkspace, StopWorkspace};
use crate::workspace_execution::{
    WorkspaceExecutionClose, WorkspaceExecutionEntry, WorkspaceExecutionTask,
};
use app_runtime_plugin::WorldEventExt;
use core_plugin::{Component, Entity, World};
use resource_id_plugin::{LibraryComponent, ResourceID};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceAgentDefinition {
    pub id: ResourceID,
    pub name: String,
    pub image: ResourceID,
    pub home_root: PathBuf,
    pub history_path: PathBuf,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceDefinition {
    pub id: ResourceID,
    pub name: String,
    pub manager: String,
    pub project_root: PathBuf,
    pub agents: Vec<WorkspaceAgentDefinition>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    InvalidRequest,
    InvalidDefinition,
    DuplicateWorkspace,
    WorkspaceNotAlive,
    WorkspaceBusy,
    ResourceResolutionFailed,
    AgentImageLoadFailed,
    AgentStartFailed,
    AgentStopFailed,
    WorkspaceNotRunning,
    ExecutionStartFailed,
    ExecutionCloseFailed,
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
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for WorkspaceError {}

#[derive(Clone, Debug)]
pub struct WorkspaceStartState {
    pub request_id: String,
    pub pending_images: BTreeMap<String, ResourceID>,
    pub image_members: BTreeMap<ResourceID, Vec<ResourceID>>,
    pub pending_agents: BTreeMap<String, ResourceID>,
    pub error: Option<WorkspaceError>,
}
#[derive(Clone, Debug)]
pub struct WorkspaceStopState {
    pub request_id: String,
    pub pending_agents: BTreeMap<String, (ResourceID, Entity)>,
    pub pending_creations: BTreeMap<String, ResourceID>,
    pub closing_execution: bool,
    pub error: Option<WorkspaceError>,
}
#[derive(Clone, Debug)]
pub enum WorkspaceState {
    Starting(WorkspaceStartState),
    Running,
    Stopping(WorkspaceStopState),
    Failed { error: WorkspaceError },
}
#[derive(Debug)]
pub struct Workspace {
    resource_id: ResourceID,
    definition: Arc<WorkspaceDefinition>,
    project_root: Arc<PathBuf>,
    agents: BTreeMap<ResourceID, Entity>,
    state: WorkspaceState,
    execution_entry: WorkspaceExecutionEntry,
    execution_close: WorkspaceExecutionClose,
    pending_creations: BTreeMap<String, ResourceID>,
    exceptions: BTreeSet<String>,
}
impl Drop for Workspace {
    fn drop(&mut self) {
        self.execution_entry.close_admission();
    }
}
impl Component for Workspace {}
impl LibraryComponent for Workspace {
    fn resource_id(&self) -> &ResourceID {
        &self.resource_id
    }
}
impl Workspace {
    pub fn new(
        definition: WorkspaceDefinition,
        state: WorkspaceState,
        execution_entry: WorkspaceExecutionEntry,
        execution_close: WorkspaceExecutionClose,
    ) -> Self {
        Self {
            resource_id: definition.id.clone(),
            project_root: Arc::new(definition.project_root.clone()),
            definition: Arc::new(definition),
            agents: BTreeMap::new(),
            state,
            execution_entry,
            execution_close,
            pending_creations: BTreeMap::new(),
            exceptions: BTreeSet::new(),
        }
    }
    pub fn definition(&self) -> &WorkspaceDefinition {
        &self.definition
    }
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
    pub fn state(&self) -> &WorkspaceState {
        &self.state
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ResourceID, Entity)> + '_ {
        self.agents.iter().map(|(id, entity)| (id, *entity))
    }
    pub fn state_mut(&mut self) -> &mut WorkspaceState {
        &mut self.state
    }
    pub fn set_state(&mut self, state: WorkspaceState) {
        self.state = state;
    }
    pub fn insert_agent(&mut self, id: ResourceID, entity: Entity) {
        self.agents.insert(id, entity);
    }
    pub fn remove_agent(&mut self, id: &ResourceID) {
        self.agents.remove(id);
    }
    pub fn close_admission(&self) {
        self.execution_entry.close_admission();
    }
    pub fn execution_close(&self) -> WorkspaceExecutionClose {
        self.execution_close.clone()
    }
    pub fn take_pending_creations(&mut self) -> BTreeMap<String, ResourceID> {
        std::mem::take(&mut self.pending_creations)
    }
    pub fn has_pending_creation(&self, id: &str) -> bool {
        self.pending_creations.contains_key(id)
    }
    pub fn consume_creation(&mut self, id: &str) -> Option<ResourceID> {
        self.pending_creations.remove(id)
    }
    pub fn record_exception(&mut self, id: String) -> bool {
        self.exceptions.insert(id)
    }
}

pub trait WorldWorkspaceExt {
    fn workspace_start(&self, id: impl Into<String>, definition: WorkspaceDefinition);
    fn workspace_stop(
        &self,
        id: impl Into<String>,
        workspace: Entity,
    ) -> Result<(), WorkspaceError>;
    fn workspace_by_id(&self, id: &ResourceID) -> Result<Entity, WorkspaceError>;
    fn workspace_list(&self) -> Vec<Entity>;
    fn workspace_submit(
        &self,
        workspace: Entity,
        task: WorkspaceExecutionTask,
    ) -> Result<(), WorkspaceError>;
    fn workspace_start_entry(
        &self,
        workspace: Entity,
        request_id: &str,
    ) -> Result<WorkspaceExecutionEntry, WorkspaceError>;
    fn workspace_accept_agent_start(
        &mut self,
        workspace: Entity,
        request: StartAgent,
    ) -> Result<(), WorkspaceError>;
}
fn absent() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::WorkspaceNotAlive,
        "workspace does not exist",
    )
}
fn not_running() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorKind::WorkspaceNotRunning,
        "workspace is not Running",
    )
}
impl WorldWorkspaceExt for World {
    fn workspace_start(&self, id: impl Into<String>, definition: WorkspaceDefinition) {
        self.send_event(StartWorkspace {
            id: id.into(),
            definition,
        });
    }
    fn workspace_stop(
        &self,
        id: impl Into<String>,
        workspace: Entity,
    ) -> Result<(), WorkspaceError> {
        if !matches!(
            self.get_component::<Workspace>(workspace)
                .ok_or_else(absent)?
                .state(),
            WorkspaceState::Running
        ) {
            return Err(not_running());
        }
        self.send_event(StopWorkspace {
            id: id.into(),
            workspace,
        });
        Ok(())
    }
    fn workspace_by_id(&self, id: &ResourceID) -> Result<Entity, WorkspaceError> {
        if id.resource_type() != "workspace" {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "expected workspace address",
            ));
        }
        let entities = id.find_entities(self).map_err(|error| {
            WorkspaceError::new(
                WorkspaceErrorKind::ResourceResolutionFailed,
                error.to_string(),
            )
        })?;
        match entities.as_slice() {
            [entity] if self.get_component::<Workspace>(*entity).is_some() => Ok(*entity),
            [] => Err(absent()),
            _ => Err(WorkspaceError::new(
                WorkspaceErrorKind::ResourceResolutionFailed,
                "workspace address is ambiguous or has wrong component",
            )),
        }
    }
    fn workspace_list(&self) -> Vec<Entity> {
        self.query_with::<Workspace>().result()
    }
    fn workspace_submit(
        &self,
        workspace: Entity,
        task: WorkspaceExecutionTask,
    ) -> Result<(), WorkspaceError> {
        let component = self
            .get_component::<Workspace>(workspace)
            .ok_or_else(absent)?;
        if !matches!(component.state(), WorkspaceState::Running) {
            return Err(not_running());
        }
        component.execution_entry.submit(task).map_err(|error| {
            WorkspaceError::new(WorkspaceErrorKind::WorkspaceNotRunning, error.to_string())
        })
    }
    fn workspace_start_entry(
        &self,
        workspace: Entity,
        request_id: &str,
    ) -> Result<WorkspaceExecutionEntry, WorkspaceError> {
        let component = self
            .get_component::<Workspace>(workspace)
            .ok_or_else(absent)?;
        let accepted = match &component.state {
            WorkspaceState::Starting(state) => state.pending_agents.contains_key(request_id),
            WorkspaceState::Running => component.pending_creations.contains_key(request_id),
            _ => false,
        };
        if !accepted {
            return Err(not_running());
        }
        Ok(component.execution_entry.clone())
    }
    fn workspace_accept_agent_start(
        &mut self,
        workspace: Entity,
        request: StartAgent,
    ) -> Result<(), WorkspaceError> {
        if self
            .query_with::<Workspace>()
            .result()
            .into_iter()
            .any(|entity| {
                self.get_component::<Workspace>(entity)
                    .is_some_and(|component| {
                        component.pending_creations.contains_key(&request.id)
                            || match component.state() {
                                WorkspaceState::Starting(state) => {
                                    state.pending_agents.contains_key(&request.id)
                                        || state.pending_images.contains_key(&request.id)
                                }
                                WorkspaceState::Stopping(state) => {
                                    state.pending_creations.contains_key(&request.id)
                                        || state.pending_agents.contains_key(&request.id)
                                }
                                _ => false,
                            }
                    })
            })
        {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceBusy,
                "request ID is already accepted",
            ));
        }
        let component = self
            .get_component_mut::<Workspace>(workspace)
            .ok_or_else(absent)?;
        if !matches!(component.state, WorkspaceState::Running)
            || component.execution_entry.is_closed()
        {
            return Err(not_running());
        }
        if request.workspace != workspace
            || request.id.is_empty()
            || request.resource_id.resource_type() != "agent"
            || request.image.resource_type() != "image"
        {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::InvalidRequest,
                "invalid dynamic agent request",
            ));
        }
        if component.pending_creations.contains_key(&request.id)
            || component
                .pending_creations
                .values()
                .any(|id| id == &request.resource_id)
            || component.agents.contains_key(&request.resource_id)
        {
            return Err(WorkspaceError::new(
                WorkspaceErrorKind::WorkspaceBusy,
                "agent creation already accepted",
            ));
        }
        component
            .pending_creations
            .insert(request.id.clone(), request.resource_id.clone());
        self.send_event(request);
        Ok(())
    }
}
