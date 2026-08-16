use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::InferenceRequestEvent;
use margatroid_types::{
    AgentContextMessagesUpdated, AgentFailure, AgentFailureKind, AgentHistoryMessageWriteRequested,
    AgentMessage, Message, ResourceId, ToolCall, ToolDefinition,
};
use tool_plugin::{
    attach_agent_tool_map, AgentToolMap, AgentToolRegisterRequest, AgentToolRegisterResponse,
    ToolCallEvent, ToolError, ToolErrorKind, ToolPluginInstalled, ToolTurnCompleted,
};

const TOOL_PERMISSION_DENIED: &str =
    "PermissionDenied: this resource is not available in the current tool schema; check the current tool schema before calling tools";

pub struct AgentPlugin {
    schedule: String,
}

impl AgentPlugin {
    pub fn new() -> Self {
        Self {
            schedule: RuntimePlugin::UPDATE.to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Default for AgentPlugin {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AgentPluginInstalled;

impl Resource for AgentPluginInstalled {}

impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("AgentPlugin requires RuntimePlugin");
        }
        if !app.world().contains_resource::<ToolPluginInstalled>() {
            panic!("AgentPlugin requires ToolPlugin");
        }
        if app.world().contains_resource::<AgentPluginInstalled>() {
            panic!("AgentPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("AgentPlugin schedule does not exist");
        }

        app.world_mut().insert_resource(AgentPluginInstalled);
        app.world_mut()
            .insert_resource(InFlightVisibilityRegistrations::default());
        app.world_mut()
            .insert_resource(PendingInferenceToolSchemas::default());
        app.add_system(&self.schedule, agent_create_system)
            .add_system(&self.schedule, agent_visibility_change_system)
            .add_system(&self.schedule, collect_agent_tool_registration_system)
            .add_system(&self.schedule, cleanup_dead_agent_registrations_system)
            .add_system(&self.schedule, agent_skill_state_system)
            .add_system(&self.schedule, agent_message_system)
            .add_system(&self.schedule, tool_turn_completed_system);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateRequest {
    pub id: String,
    pub agent_id: ResourceId,
    pub workspace_id: Entity,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tool_context: Vec<Message>,
    pub default_visibility: BTreeSet<ResourceId>,
}

impl Event for AgentCreateRequest {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateResult {
    pub id: String,
    pub agent_id: ResourceId,
    pub result: Result<Entity, AgentCreateError>,
}

impl Event for AgentCreateResult {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreated {
    pub id: String,
    pub agent_id: ResourceId,
    pub agent: Entity,
}

impl Event for AgentCreated {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectAgentVisibleResource {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for InjectAgentVisibleResource {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveAgentVisibleResource {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for RemoveAgentVisibleResource {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetAgentDefaultResourceVisibility {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub visible: bool,
}
impl Event for SetAgentDefaultResourceVisibility {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreAgentDefaultVisibility {
    pub id: String,
    pub agent: Entity,
}
impl Event for RestoreAgentDefaultVisibility {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveAllAgentVisibleResources {
    pub id: String,
    pub agent: Entity,
}
impl Event for RemoveAllAgentVisibleResources {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentVisibleResourceInjected {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for AgentVisibleResourceInjected {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentVisibleResourceRemoved {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}
impl Event for AgentVisibleResourceRemoved {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentVisibleResourceInjectionFailed {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
    pub error: AgentVisibilityError,
}
impl Event for AgentVisibleResourceInjectionFailed {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentCreateErrorKind {
    InvalidRequest,
    DuplicateAgent,
    WorkspaceMissing,
    ContextInvalid,
    ToolMapSetupFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCreateError {
    kind: AgentCreateErrorKind,
    message: String,
}

impl AgentCreateError {
    fn new(kind: AgentCreateErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: bounded_message(message.into()),
        }
    }
    pub fn kind(&self) -> AgentCreateErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AgentCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for AgentCreateError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentVisibilityErrorKind {
    AgentMissing,
    VisibilityMissing,
    ToolMapMissing,
    RegistrationFailed,
    RegistrationResponseMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentVisibilityError {
    kind: AgentVisibilityErrorKind,
    message: String,
}

impl AgentVisibilityError {
    fn new(kind: AgentVisibilityErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: bounded_message(message.into()),
        }
    }
    pub fn kind(&self) -> AgentVisibilityErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AgentVisibilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for AgentVisibilityError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadAgentSkill {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}

impl Event for LoadAgentSkill {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnloadAgentSkill {
    pub id: String,
    pub agent: Entity,
    pub resource_id: ResourceId,
}

impl Event for UnloadAgentSkill {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnloadAllAgentSkills {
    pub id: String,
    pub agent: Entity,
}

impl Event for UnloadAllAgentSkills {}

pub struct AgentIdentity {
    id: ResourceId,
}

impl AgentIdentity {
    pub fn id(&self) -> &ResourceId {
        &self.id
    }
}

impl Component for AgentIdentity {}

pub trait WorldAgentExt {
    fn agent(&self, id: &ResourceId) -> Option<Entity>;
    fn agent_loading_skills(&self, agent: Entity) -> Option<&BTreeSet<ResourceId>>;
    fn inject_agent_visible_resource(
        &self,
        id: impl Into<String>,
        agent: Entity,
        resource_id: ResourceId,
    );
    fn remove_agent_visible_resource(
        &self,
        id: impl Into<String>,
        agent: Entity,
        resource_id: ResourceId,
    );
    fn restore_agent_default_visibility(&self, id: impl Into<String>, agent: Entity);
    fn remove_all_agent_visible_resources(&self, id: impl Into<String>, agent: Entity);
}

impl WorldAgentExt for World {
    fn agent(&self, id: &ResourceId) -> Option<Entity> {
        self.query_with::<AgentIdentity>()
            .result()
            .into_iter()
            .find(|entity| {
                self.get_component::<AgentIdentity>(*entity)
                    .is_some_and(|identity| identity.id() == id)
            })
    }

    fn agent_loading_skills(&self, agent: Entity) -> Option<&BTreeSet<ResourceId>> {
        self.get_component::<AgentStatus>(agent)
            .map(|status| &status.loading_skills)
    }

    fn inject_agent_visible_resource(
        &self,
        id: impl Into<String>,
        agent: Entity,
        resource_id: ResourceId,
    ) {
        self.send_event(InjectAgentVisibleResource {
            id: id.into(),
            agent,
            resource_id,
        });
    }

    fn remove_agent_visible_resource(
        &self,
        id: impl Into<String>,
        agent: Entity,
        resource_id: ResourceId,
    ) {
        self.send_event(RemoveAgentVisibleResource {
            id: id.into(),
            agent,
            resource_id,
        });
    }

    fn restore_agent_default_visibility(&self, id: impl Into<String>, agent: Entity) {
        self.send_event(RestoreAgentDefaultVisibility {
            id: id.into(),
            agent,
        });
    }

    fn remove_all_agent_visible_resources(&self, id: impl Into<String>, agent: Entity) {
        self.send_event(RemoveAllAgentVisibleResources {
            id: id.into(),
            agent,
        });
    }
}

pub struct AgentWorkspaceId {
    workspace_id: Entity,
}

impl AgentWorkspaceId {
    pub fn workspace_id(&self) -> Entity {
        self.workspace_id
    }
}

impl Component for AgentWorkspaceId {}

pub struct AgentContext {
    system_prompt: String,
    messages: Vec<Message>,
    tool_context: Vec<Message>,
}

impl AgentContext {
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tool_context(&self) -> &[Message] {
        &self.tool_context
    }

    pub fn append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender) {
        assert_conversation_message(&message);
        self.messages.push(message);
        self.notify_updated(agent, events);
    }

    pub fn rewrite_messages(
        &mut self,
        agent: Entity,
        messages: Vec<Message>,
        events: &RuntimeEventSender,
    ) {
        assert_conversation_messages(&messages);
        self.messages = messages;
        self.notify_updated(agent, events);
    }

    pub fn append_tool_context(
        &mut self,
        agent: Entity,
        message: Message,
        events: &RuntimeEventSender,
    ) {
        assert!(matches!(message, Message::Tool { .. }));
        self.tool_context.push(message);
        self.notify_updated(agent, events);
    }

    pub fn clear_tool_context(&mut self, agent: Entity, events: &RuntimeEventSender) {
        if self.tool_context.is_empty() {
            return;
        }
        self.tool_context.clear();
        self.notify_updated(agent, events);
    }

    fn notify_updated(&self, agent: Entity, events: &RuntimeEventSender) {
        events.send_event(AgentContextMessagesUpdated {
            agent,
            messages: self.messages.clone(),
            tool_context: self.tool_context.clone(),
        });
    }
}

impl Component for AgentContext {}

pub struct AgentDefaultVisibility {
    resources: BTreeSet<ResourceId>,
}

impl AgentDefaultVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceId> {
        &self.resources
    }
}

impl Component for AgentDefaultVisibility {}

pub struct AgentDynamicVisibility {
    resources: BTreeSet<ResourceId>,
}

impl AgentDynamicVisibility {
    pub fn resources(&self) -> &BTreeSet<ResourceId> {
        &self.resources
    }
}

impl Component for AgentDynamicVisibility {}

#[derive(Default)]
struct InFlightVisibilityRegistrations {
    registrations: HashMap<(Entity, ResourceId), InFlightVisibilityRegistration>,
}
impl Resource for InFlightVisibilityRegistrations {}

struct InFlightVisibilityRegistration {
    registration_id: String,
    agent: Entity,
    resource_id: ResourceId,
    notification_ids: BTreeSet<String>,
    desired_visible: bool,
}

#[derive(Default)]
struct PendingInferenceToolSchemas {
    schemas: HashMap<(Entity, String), Vec<ToolDefinition>>,
}

impl Resource for PendingInferenceToolSchemas {}

#[derive(Default)]
pub(crate) struct AgentStatus {
    turn_id: Option<String>,
    loading_skills: BTreeSet<ResourceId>,
}

impl AgentStatus {
    pub(crate) fn begin_turn(&mut self, turn_id: String) -> Result<(), AgentStepError> {
        if self.turn_id.is_some() {
            return Err(AgentStepError::InvalidToolBatch);
        }
        self.turn_id = Some(turn_id);
        Ok(())
    }
    pub(crate) fn finish_turn(&mut self, turn_id: &str) -> Result<(), AgentStepError> {
        if self.turn_id.as_deref() != Some(turn_id) {
            return Err(AgentStepError::InvalidToolBatch);
        }
        self.turn_id = None;
        Ok(())
    }
    pub(crate) fn load_skill(&mut self, resource_id: ResourceId) -> Result<(), AgentStepError> {
        if resource_id.resource_type() != "skill" {
            return Err(AgentStepError::InvalidToolBatch);
        }
        self.loading_skills.insert(resource_id);
        Ok(())
    }
    pub(crate) fn unload_skill(&mut self, resource_id: &ResourceId) -> bool {
        self.loading_skills.remove(resource_id)
    }
    pub(crate) fn unload_all_skills(&mut self) {
        self.loading_skills.clear();
    }
}

impl Component for AgentStatus {}

struct AvailableTools {
    definitions: Vec<ToolDefinition>,
}

enum ConversationTurnResult {
    WaitForTools,
    FinishTurn,
    RequestInference,
}

enum AgentStepError {
    AgentMissing,
    IdentityMissing,
    ContextMissing,
    StatusMissing,
    ToolMapMissing,
    InvalidMessage,
    InvalidToolBatch,
    Tool(ToolError),
}

impl AgentStepError {
    fn failure_message(&self) -> String {
        match self {
            Self::AgentMissing => "AgentMissing: agent entity is not alive".into(),
            Self::IdentityMissing => "IdentityMissing: agent identity is missing".into(),
            Self::ContextMissing => "ContextMissing: agent context is missing".into(),
            Self::StatusMissing => "StatusMissing: agent status is missing".into(),
            Self::ToolMapMissing => "ToolMapMissing: Agent tool map is missing".into(),
            Self::InvalidMessage => "InvalidMessage: message type is invalid".into(),
            Self::InvalidToolBatch => "InvalidToolBatch: tool call batch is invalid".into(),
            Self::Tool(error) => error.to_string(),
        }
    }
}

fn agent_create_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentCreateRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for request in requests {
        let result = create_agent(world, &request);
        if let Ok(agent) = result {
            events.send_event(AgentCreated {
                id: request.id.clone(),
                agent_id: request.agent_id.clone(),
                agent,
            });
            world.restore_agent_default_visibility(
                format!("agent-{}/restore-default", agent.index()),
                agent,
            );
        }
        events.send_event(AgentCreateResult {
            id: request.id,
            agent_id: request.agent_id,
            result,
        });
    }
}

fn create_agent(
    world: &mut World,
    request: &AgentCreateRequest,
) -> Result<Entity, AgentCreateError> {
    if request.id.is_empty() || request.agent_id.resource_type() != "agent" {
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::InvalidRequest,
            "Agent create request is invalid",
        ));
    }
    if world.agent(&request.agent_id).is_some() {
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::DuplicateAgent,
            "Agent resource ID is already alive",
        ));
    }
    if !world.is_alive(request.workspace_id) {
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::WorkspaceMissing,
            "Workspace entity is not alive",
        ));
    }
    if request
        .messages
        .iter()
        .any(|message| !matches!(message, Message::User { .. } | Message::Assistant { .. }))
        || request
            .tool_context
            .iter()
            .any(|message| !matches!(message, Message::Tool { .. }))
    {
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::ContextInvalid,
            "Recovered Agent context contains an invalid message type",
        ));
    }

    let agent = world.spawn();
    let inserted = world.insert_component(
        agent,
        AgentIdentity {
            id: request.agent_id.clone(),
        },
    ) && world.insert_component(
        agent,
        AgentWorkspaceId {
            workspace_id: request.workspace_id,
        },
    ) && world.insert_component(
        agent,
        AgentContext {
            system_prompt: request.system_prompt.clone(),
            messages: request.messages.clone(),
            tool_context: request.tool_context.clone(),
        },
    ) && world.insert_component(
        agent,
        AgentDefaultVisibility {
            resources: request.default_visibility.clone(),
        },
    ) && world.insert_component(
        agent,
        AgentDynamicVisibility {
            resources: BTreeSet::new(),
        },
    ) && world.insert_component(agent, AgentStatus::default());
    if !inserted {
        world.despawn(agent);
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::InvalidRequest,
            "Agent components could not be attached",
        ));
    }
    if let Err(error) = attach_agent_tool_map(world, agent) {
        world.despawn(agent);
        return Err(AgentCreateError::new(
            AgentCreateErrorKind::ToolMapSetupFailed,
            error.to_string(),
        ));
    }
    Ok(agent)
}

fn agent_visibility_change_system(world: &mut World) {
    let injects = world
        .event_reader::<InjectAgentVisibleResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let removes = world
        .event_reader::<RemoveAgentVisibleResource>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let restores = world
        .event_reader::<RestoreAgentDefaultVisibility>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let default_changes = world
        .event_reader::<SetAgentDefaultResourceVisibility>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let remove_all = world
        .event_reader::<RemoveAllAgentVisibleResources>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for event in injects {
        inject_visible_resource(world, &event.id, event.agent, &event.resource_id, &events);
    }
    for event in removes {
        if let Err(error) =
            remove_visible_resource(world, &event.id, event.agent, &event.resource_id, &events)
        {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
    for event in default_changes {
        let is_default = world
            .get_component::<AgentDefaultVisibility>(event.agent)
            .is_some_and(|visibility| visibility.resources.contains(&event.resource_id));
        if !is_default {
            events.send_event(AgentFailure {
                id: event.id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: "InvalidRequest: resource is not part of Agent default visibility".into(),
            });
            continue;
        }
        if event.visible {
            inject_visible_resource(world, &event.id, event.agent, &event.resource_id, &events);
        } else if let Err(error) =
            remove_visible_resource(world, &event.id, event.agent, &event.resource_id, &events)
        {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
    for event in restores {
        let resources = world
            .get_component::<AgentDefaultVisibility>(event.agent)
            .map(|visibility| visibility.resources.iter().cloned().collect::<Vec<_>>());
        let Some(resources) = resources else {
            report_visibility_operation_failure(
                &event.id,
                event.agent,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    "Agent default visibility is missing",
                ),
                &events,
            );
            continue;
        };
        if let Err(error) =
            remove_all_visible_resources(world, &event.id, event.agent, false, &events)
        {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
            continue;
        }
        for resource_id in resources {
            inject_visible_resource(world, &event.id, event.agent, &resource_id, &events);
        }
    }
    for event in remove_all {
        if let Err(error) =
            remove_all_visible_resources(world, &event.id, event.agent, true, &events)
        {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
}

fn inject_visible_resource(
    world: &mut World,
    id: &str,
    agent: Entity,
    resource_id: &ResourceId,
    events: &RuntimeEventSender,
) {
    if !world.is_alive(agent) {
        send_visibility_injection_failed(
            id,
            agent,
            resource_id,
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::AgentMissing,
                "Agent entity is not alive",
            ),
            events,
        );
        return;
    }
    let Some(visibility) = world.get_component::<AgentDynamicVisibility>(agent) else {
        send_visibility_injection_failed(
            id,
            agent,
            resource_id,
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                "Agent dynamic visibility is missing",
            ),
            events,
        );
        return;
    };
    if visibility.resources.contains(resource_id) {
        events.send_event(AgentVisibleResourceInjected {
            id: id.to_owned(),
            agent,
            resource_id: resource_id.clone(),
        });
        return;
    }
    let Some(tool_map) = world.get_component::<AgentToolMap>(agent) else {
        send_visibility_injection_failed(
            id,
            agent,
            resource_id,
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::ToolMapMissing,
                "Agent tool map is missing",
            ),
            events,
        );
        return;
    };
    let mapping_count = tool_map.get_by_resource(resource_id).len();
    if mapping_count > 1 {
        send_visibility_injection_failed(
            id,
            agent,
            resource_id,
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::RegistrationResponseMismatch,
                "Agent tool map contains duplicate resource mappings",
            ),
            events,
        );
        return;
    }
    if mapping_count == 1 {
        world
            .get_component_mut::<AgentDynamicVisibility>(agent)
            .expect("AgentDynamicVisibility existence was checked")
            .resources
            .insert(resource_id.clone());
        events.send_event(AgentVisibleResourceInjected {
            id: id.to_owned(),
            agent,
            resource_id: resource_id.clone(),
        });
        return;
    }

    let key = (agent, resource_id.clone());
    let registrations = world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed");
    if let Some(registration) = registrations.registrations.get_mut(&key) {
        registration.notification_ids.insert(id.to_owned());
        registration.desired_visible = true;
        return;
    }
    static NEXT_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);
    let registration_id = format!(
        "agent-tool-registration-{}",
        NEXT_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut notification_ids = BTreeSet::new();
    notification_ids.insert(id.to_owned());
    registrations.registrations.insert(
        key,
        InFlightVisibilityRegistration {
            registration_id: registration_id.clone(),
            agent,
            resource_id: resource_id.clone(),
            notification_ids,
            desired_visible: true,
        },
    );
    events.send_event(AgentToolRegisterRequest {
        id: registration_id,
        agent,
        resource_id: resource_id.clone(),
    });
}

fn remove_visible_resource(
    world: &mut World,
    id: &str,
    agent: Entity,
    resource_id: &ResourceId,
    events: &RuntimeEventSender,
) -> Result<(), AgentVisibilityError> {
    if !world.is_alive(agent) {
        return Err(AgentVisibilityError::new(
            AgentVisibilityErrorKind::AgentMissing,
            "Agent entity is not alive",
        ));
    }
    world
        .get_component_mut::<AgentDynamicVisibility>(agent)
        .ok_or_else(|| {
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                "Agent dynamic visibility is missing",
            )
        })?
        .resources
        .remove(resource_id);
    if resource_id.resource_type() == "skill" {
        world
            .get_component_mut::<AgentStatus>(agent)
            .ok_or_else(|| {
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    "Agent status is missing",
                )
            })?
            .unload_skill(resource_id);
    }
    if let Some(registration) = world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed")
        .registrations
        .get_mut(&(agent, resource_id.clone()))
    {
        registration.desired_visible = false;
    }
    events.send_event(AgentVisibleResourceRemoved {
        id: id.to_owned(),
        agent,
        resource_id: resource_id.clone(),
    });
    Ok(())
}

fn remove_all_visible_resources(
    world: &mut World,
    id: &str,
    agent: Entity,
    clear_loading_skills: bool,
    events: &RuntimeEventSender,
) -> Result<(), AgentVisibilityError> {
    if !world.is_alive(agent) {
        return Err(AgentVisibilityError::new(
            AgentVisibilityErrorKind::AgentMissing,
            "Agent entity is not alive",
        ));
    }
    let removed = {
        let visibility = world
            .get_component_mut::<AgentDynamicVisibility>(agent)
            .ok_or_else(|| {
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    "Agent dynamic visibility is missing",
                )
            })?;
        std::mem::take(&mut visibility.resources)
    };
    for registration in world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed")
        .registrations
        .values_mut()
        .filter(|registration| registration.agent == agent)
    {
        registration.desired_visible = false;
    }
    if clear_loading_skills {
        world
            .get_component_mut::<AgentStatus>(agent)
            .ok_or_else(|| {
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    "Agent status is missing",
                )
            })?
            .unload_all_skills();
    }
    for resource_id in removed {
        events.send_event(AgentVisibleResourceRemoved {
            id: id.to_owned(),
            agent,
            resource_id,
        });
    }
    Ok(())
}

fn collect_agent_tool_registration_system(world: &mut World) {
    let responses = world
        .event_reader::<AgentToolRegisterResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for response in responses {
        let key = world
            .get_resource::<InFlightVisibilityRegistrations>()
            .expect("AgentPlugin is not installed")
            .registrations
            .iter()
            .find_map(|(key, registration)| {
                (registration.registration_id == response.id).then(|| key.clone())
            });
        let Some(key) = key else {
            tracing::warn!(registration_id = %response.id, "unmatched Agent tool registration response");
            continue;
        };
        let registration = world
            .get_resource_mut::<InFlightVisibilityRegistrations>()
            .expect("AgentPlugin is not installed")
            .registrations
            .remove(&key)
            .expect("registration key was just found");
        if registration.agent != response.agent || registration.resource_id != response.resource_id
        {
            notify_registration_failure(
                &registration,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::RegistrationResponseMismatch,
                    "Agent tool registration response does not match its request",
                ),
                &events,
            );
            continue;
        }
        if let Err(error) = response.result {
            notify_registration_failure(
                &registration,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::RegistrationFailed,
                    error.to_string(),
                ),
                &events,
            );
            continue;
        }
        let valid_mapping = world.is_alive(registration.agent)
            && world
                .get_component::<AgentToolMap>(registration.agent)
                .is_some_and(|map| map.get_by_resource(&registration.resource_id).len() == 1);
        if !valid_mapping {
            notify_registration_failure(
                &registration,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::RegistrationResponseMismatch,
                    "Agent tool registration succeeded without one matching tool map",
                ),
                &events,
            );
            continue;
        }
        if !registration.desired_visible {
            continue;
        }
        let Some(visibility) =
            world.get_component_mut::<AgentDynamicVisibility>(registration.agent)
        else {
            notify_registration_failure(
                &registration,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    "Agent dynamic visibility disappeared before registration completed",
                ),
                &events,
            );
            continue;
        };
        visibility
            .resources
            .insert(registration.resource_id.clone());
        for id in &registration.notification_ids {
            events.send_event(AgentVisibleResourceInjected {
                id: id.clone(),
                agent: registration.agent,
                resource_id: registration.resource_id.clone(),
            });
        }
    }
}

fn cleanup_dead_agent_registrations_system(world: &mut World) {
    let dead = world
        .get_resource::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed")
        .registrations
        .keys()
        .filter(|(agent, _)| !world.is_alive(*agent))
        .cloned()
        .collect::<Vec<_>>();
    let registrations = world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed");
    for key in dead {
        registrations.registrations.remove(&key);
    }
    let dead_schemas = world
        .get_resource::<PendingInferenceToolSchemas>()
        .expect("AgentPlugin is not installed")
        .schemas
        .keys()
        .filter(|(agent, _)| !world.is_alive(*agent))
        .cloned()
        .collect::<Vec<_>>();
    let schemas = world
        .get_resource_mut::<PendingInferenceToolSchemas>()
        .expect("AgentPlugin is not installed");
    for key in dead_schemas {
        schemas.schemas.remove(&key);
    }
}

fn notify_registration_failure(
    registration: &InFlightVisibilityRegistration,
    error: AgentVisibilityError,
    events: &RuntimeEventSender,
) {
    for id in &registration.notification_ids {
        send_visibility_injection_failed(
            id,
            registration.agent,
            &registration.resource_id,
            error.clone(),
            events,
        );
    }
}

fn send_visibility_injection_failed(
    id: &str,
    agent: Entity,
    resource_id: &ResourceId,
    error: AgentVisibilityError,
    events: &RuntimeEventSender,
) {
    tracing::warn!(request_id = id, ?agent, resource = %resource_id, error = %error, "Agent visible resource injection failed");
    events.send_event(AgentVisibleResourceInjectionFailed {
        id: id.to_owned(),
        agent,
        resource_id: resource_id.clone(),
        error,
    });
}

fn report_visibility_operation_failure(
    id: &str,
    agent: Entity,
    error: AgentVisibilityError,
    events: &RuntimeEventSender,
) {
    events.send_event(AgentFailure {
        id: id.to_owned(),
        agent,
        kind: AgentFailureKind::Agent,
        message: error.to_string(),
    });
}

fn agent_skill_state_system(world: &mut World) {
    let loads = world
        .event_reader::<LoadAgentSkill>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let unloads = world
        .event_reader::<UnloadAgentSkill>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let unload_all = world
        .event_reader::<UnloadAllAgentSkills>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for event in loads {
        let result = world
            .get_component_mut::<AgentStatus>(event.agent)
            .ok_or(AgentStepError::AgentMissing)
            .and_then(|status| status.load_skill(event.resource_id));
        if let Err(error) = result {
            events.send_event(AgentFailure {
                id: event.id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }

    for event in unloads {
        let result = if event.resource_id.resource_type() != "skill" {
            Err(AgentStepError::InvalidToolBatch)
        } else {
            world
                .get_component_mut::<AgentStatus>(event.agent)
                .ok_or(AgentStepError::AgentMissing)
                .map(|status| {
                    status.unload_skill(&event.resource_id);
                })
        };
        if let Err(error) = result {
            events.send_event(AgentFailure {
                id: event.id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }

    for event in unload_all {
        let result = world
            .get_component_mut::<AgentStatus>(event.agent)
            .ok_or(AgentStepError::AgentMissing)
            .map(|status| status.unload_all_skills());
        if let Err(error) = result {
            events.send_event(AgentFailure {
                id: event.id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }
}

fn agent_message_system(world: &mut World) {
    let messages = world
        .event_reader::<AgentMessage>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for message in messages {
        let agent = message.agent;
        if !world.is_alive(agent) {
            tracing::warn!(id = %message.id, "AgentMessage agent does not exist");
            continue;
        }
        match handle_agent_message(world, &message, &events) {
            Ok(_) => {}
            Err(error) => {
                events.send_event(AgentFailure {
                    id: message.id.clone(),
                    agent,
                    kind: AgentFailureKind::Agent,
                    message: error.failure_message(),
                });
            }
        }
    }
}

fn handle_agent_message(
    world: &mut World,
    event: &AgentMessage,
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let agent = event.agent;
    let result = match &event.message {
        Message::System { .. } => return Err(AgentStepError::InvalidMessage),
        Message::User { tool_calls, .. } => {
            world
                .get_component_mut::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .begin_turn(event.id.clone())?;
            clear_tool_context(world, agent, events)?;
            record_history_message(world, event, events, Vec::new());
            append_conversation_message(world, agent, event.message.clone(), events)?;
            dispatch_tool_calls(world, &event.id, agent, tool_calls, true, events)?
        }
        Message::Assistant { tool_calls, .. } => {
            if world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .turn_id
                .as_deref()
                != Some(&event.id)
            {
                return Err(AgentStepError::InvalidToolBatch);
            }
            let tool_schema = take_pending_tool_schema(world, agent, &event.id);
            clear_tool_context(world, agent, events)?;
            record_history_message(world, event, events, tool_schema.clone());
            append_conversation_message(world, agent, event.message.clone(), events)?;
            if !tool_calls.is_empty() {
                dispatch_assistant_tool_calls(
                    world,
                    &event.id,
                    agent,
                    tool_calls,
                    &tool_schema,
                    events,
                )?
            } else {
                world
                    .get_component_mut::<AgentStatus>(agent)
                    .ok_or(AgentStepError::StatusMissing)?
                    .finish_turn(&event.id)?;
                ConversationTurnResult::FinishTurn
            }
        }
        Message::Tool { resource_id: _, .. } => {
            if world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .turn_id
                .as_deref()
                != Some(&event.id)
            {
                return Err(AgentStepError::InvalidToolBatch);
            }
            record_history_message(world, event, events, Vec::new());
            append_tool_context(world, agent, event.message.clone(), events)?;
            ConversationTurnResult::WaitForTools
        }
    };
    if matches!(result, ConversationTurnResult::RequestInference) {
        send_inference_request(world, &event.id, agent, events)?;
    }
    Ok(result)
}

fn take_pending_tool_schema(
    world: &mut World,
    agent: Entity,
    turn_id: &str,
) -> Vec<ToolDefinition> {
    world
        .get_resource_mut::<PendingInferenceToolSchemas>()
        .expect("AgentPlugin is not installed")
        .schemas
        .remove(&(agent, turn_id.to_owned()))
        .unwrap_or_default()
}

fn record_history_message(
    _world: &mut World,
    event: &AgentMessage,
    events: &RuntimeEventSender,
    tool_schema: Vec<ToolDefinition>,
) {
    let message = match &event.message {
        Message::Tool {
            resource_id,
            tool_call_id,
            ..
        } => Message::Tool {
            resource_id: resource_id.clone(),
            tool_call_id: tool_call_id.clone(),
            content: resource_id.to_string(),
        },
        _ => event.message.clone(),
    };
    events.send_event(AgentHistoryMessageWriteRequested {
        id: event.id.clone(),
        agent: event.agent,
        message,
        tool_schema,
    });
}

fn append_conversation_message(
    world: &mut World,
    agent: Entity,
    message: Message,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    if !world.is_alive(agent) {
        return Err(AgentStepError::AgentMissing);
    }
    if world.get_component::<AgentContext>(agent).is_none() {
        return Err(AgentStepError::ContextMissing);
    }
    world
        .get_component_mut::<AgentContext>(agent)
        .expect("AgentContext existence was checked")
        .append_message(agent, message, events);
    Ok(())
}

fn append_tool_context(
    world: &mut World,
    agent: Entity,
    message: Message,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    world
        .get_component_mut::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .append_tool_context(agent, message, events);
    Ok(())
}

fn clear_tool_context(
    world: &mut World,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    world
        .get_component_mut::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .clear_tool_context(agent, events);
    Ok(())
}

fn dispatch_tool_calls(
    world: &World,
    id: &str,
    agent: Entity,
    tool_calls: &[ToolCall],
    include_loading_skills: bool,
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    let mut calls = tool_calls.to_vec();
    if include_loading_skills {
        calls.extend(expand_loading_skills(world, agent)?);
    }
    if calls.is_empty() {
        return Ok(ConversationTurnResult::RequestInference);
    }
    let mut call_ids = BTreeSet::new();
    if calls.iter().any(|call| {
        call.id.is_empty()
            || call.tool_name.is_empty()
            || maps.get_by_name(&call.tool_name).is_none()
            || !call_ids.insert(call.id.clone())
    }) {
        return Err(AgentStepError::InvalidToolBatch);
    }
    for call in calls {
        events.send_event(ToolCallEvent {
            turn_id: id.to_owned(),
            agent,
            call,
        });
    }
    Ok(ConversationTurnResult::WaitForTools)
}

fn dispatch_assistant_tool_calls(
    world: &mut World,
    id: &str,
    agent: Entity,
    tool_calls: &[ToolCall],
    tool_schema: &[ToolDefinition],
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    let visibility = world
        .get_component::<AgentDynamicVisibility>(agent)
        .ok_or(AgentStepError::ContextMissing)?;
    let schema_names = tool_schema
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut call_ids = BTreeSet::new();
    let mut authorized = Vec::new();
    let mut authorized_skills = Vec::new();
    let mut denied = Vec::new();
    for call in tool_calls {
        if call.id.is_empty() || call.tool_name.is_empty() || !call_ids.insert(call.id.clone()) {
            return Err(AgentStepError::InvalidToolBatch);
        }
        let map = maps
            .get_by_name(&call.tool_name)
            .ok_or(AgentStepError::InvalidToolBatch)?;
        if !schema_names.contains(call.tool_name.as_str())
            || !visibility.resources().contains(&map.resource_id)
        {
            denied.push((call.id.clone(), map.resource_id.clone()));
        } else if map.resource_id.resource_type() == "skill" {
            authorized_skills.push(map.resource_id.clone());
        } else {
            authorized.push(call.clone());
        }
    }
    if !authorized_skills.is_empty() {
        let status = world
            .get_component_mut::<AgentStatus>(agent)
            .ok_or(AgentStepError::StatusMissing)?;
        for resource_id in authorized_skills {
            status.load_skill(resource_id)?;
        }
    }
    let denied_only = !denied.is_empty() && authorized.is_empty();
    for (tool_call_id, resource_id) in denied {
        events.send_event(AgentMessage {
            id: id.to_owned(),
            agent,
            message: Message::Tool {
                resource_id,
                tool_call_id,
                content: TOOL_PERMISSION_DENIED.into(),
            },
        });
    }
    let has_loading_skills = !world
        .get_component::<AgentStatus>(agent)
        .ok_or(AgentStepError::StatusMissing)?
        .loading_skills
        .is_empty();
    let result = dispatch_tool_calls(world, id, agent, &authorized, true, events)?;
    if !denied_only || has_loading_skills {
        return Ok(result);
    }
    events.send_event(ToolTurnCompleted {
        turn_id: id.to_owned(),
        agent,
    });
    Ok(ConversationTurnResult::WaitForTools)
}

fn expand_loading_skills(world: &World, agent: Entity) -> Result<Vec<ToolCall>, AgentStepError> {
    static NEXT_SKILL_CALL_ID: AtomicU64 = AtomicU64::new(1);
    let status = world
        .get_component::<AgentStatus>(agent)
        .ok_or(AgentStepError::StatusMissing)?;
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    status
        .loading_skills
        .iter()
        .map(|resource_id| {
            let matches = maps.get_by_resource(resource_id);
            if matches.len() != 1 {
                return Err(AgentStepError::InvalidToolBatch);
            }
            Ok(ToolCall {
                id: format!(
                    "loading-skill-{}",
                    NEXT_SKILL_CALL_ID.fetch_add(1, Ordering::Relaxed)
                ),
                tool_name: matches[0].tool_name.clone(),
                arguments: "{}".into(),
            })
        })
        .collect()
}

fn send_inference_request(
    world: &mut World,
    id: &str,
    agent: Entity,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let available_tools = build_available_tools(world, agent)?;
    let messages = build_inference_context(world, agent)?;
    let agent_id = world
        .get_component::<AgentIdentity>(agent)
        .map(|identity| identity.id().clone())
        .ok_or(AgentStepError::IdentityMissing)?;
    tracing::info!(
        request_id = id,
        agent = %agent_id,
        messages = messages.len(),
        tools = available_tools.definitions.len(),
        "inference requested"
    );
    world
        .get_resource_mut::<PendingInferenceToolSchemas>()
        .expect("AgentPlugin is not installed")
        .schemas
        .insert((agent, id.to_owned()), available_tools.definitions.clone());
    events.send_event(InferenceRequestEvent {
        id: id.to_owned(),
        agent,
        agent_id,
        messages,
        tools: available_tools.definitions,
    });
    Ok(())
}

fn build_inference_context(world: &World, agent: Entity) -> Result<Vec<Message>, AgentStepError> {
    let context = world
        .get_component::<AgentContext>(agent)
        .ok_or(AgentStepError::ContextMissing)?;
    let mut messages = Vec::with_capacity(context.messages.len() + context.tool_context.len() + 1);
    messages.push(Message::System {
        content: context.system_prompt.clone(),
    });
    messages.extend(context.messages.iter().cloned());
    messages.extend(context.tool_context.iter().cloned());
    Ok(messages)
}

fn build_available_tools(world: &World, agent: Entity) -> Result<AvailableTools, AgentStepError> {
    let resources = world
        .get_component::<AgentDynamicVisibility>(agent)
        .ok_or(AgentStepError::ContextMissing)?
        .resources()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    let mut definitions = Vec::with_capacity(resources.len());
    for resource in &resources {
        let matched = maps.get_by_resource(resource);
        if matched.len() != 1 {
            return Err(AgentStepError::Tool(ToolError::new(
                ToolErrorKind::ProviderMissing,
                "visible resource is not registered exactly once",
            )));
        }
        definitions.push(ToolDefinition {
            name: matched[0].template.name.clone(),
            description: matched[0].template.description.clone(),
            input_schema: matched[0].template.parameters.clone(),
        });
    }
    Ok(AvailableTools { definitions })
}

fn tool_turn_completed_system(world: &mut World) {
    let completed = world
        .event_reader::<ToolTurnCompleted>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for event in completed {
        let current_turn = world
            .get_component::<AgentStatus>(event.agent)
            .ok_or(AgentStepError::StatusMissing)
            .map(|status| status.turn_id.as_deref() == Some(&event.turn_id));
        let result = match current_turn {
            Ok(true) => send_inference_request(world, &event.turn_id, event.agent, &events),
            Ok(false) => Err(AgentStepError::InvalidToolBatch),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            events.send_event(AgentFailure {
                id: event.turn_id,
                agent: event.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }
}

fn assert_conversation_message(message: &Message) {
    assert!(
        matches!(message, Message::User { .. } | Message::Assistant { .. }),
        "AgentContext conversation messages must be User or Assistant"
    );
}

fn assert_conversation_messages(messages: &[Message]) {
    for message in messages {
        assert_conversation_message(message);
    }
}

fn bounded_message(mut message: String) -> String {
    const LIMIT: usize = 512;
    if message.len() <= LIMIT {
        return message;
    }
    let mut boundary = LIMIT - 3;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message.push_str("...");
    message
}

#[cfg(all(test, any()))]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use async_runtime_plugin::AsyncRuntimePlugin;
    use margatroid_types::ToolDefinition;
    use memory_plugin::{AgentMemory, MemoryPlugin, WorldMemoryExt};
    use serde_json::json;
    use tempfile::tempdir;
    use tool_plugin::{AgentToolEnvironment, AppToolExt, ToolPlugin, ToolTemplate};

    use super::*;

    fn resource(name: &str) -> ResourceId {
        ResourceId::parse(format!("tool:{name}")).unwrap()
    }

    fn tool(resource: ResourceId, exposed_name: &str) -> ToolTemplate {
        ToolTemplate::new(
            resource,
            ToolDefinition {
                name: exposed_name.into(),
                description: "Test tool".into(),
                input_schema: json!({"type":"object"}),
            },
        )
        .unwrap()
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ToolPlugin::default())
            .add_plugin(MemoryPlugin::default())
            .add_plugin(AgentPlugin::default());
        app
    }

    fn create_agent(app: &mut App, visibility: BTreeSet<ResourceId>) -> Entity {
        let workspace = app.world_mut().spawn();
        app.world().send_event(AgentCreateRequest {
            id: "agent-1".into(),
            agent_id: ResourceId::parse("agent:test/agent0").unwrap(),
            workspace_id: workspace,
            system_prompt: "system".into(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            default_visibility: visibility,
        });
        app.tick();
        app.tick();
        let agent = app
            .world()
            .event_reader::<AgentCreated>()
            .into_iter()
            .next()
            .unwrap()
            .agent;
        let directory = tempdir().unwrap();
        let path = directory.keep().join("memory.sql");
        let (memory, restored) = AgentMemory::open(path).unwrap();
        app.world_mut()
            .bind_agent_memory(agent, memory, &restored)
            .unwrap();
        app.world_mut().insert_component(
            agent,
            AgentToolEnvironment::new(Path::new("/project"), Path::new("/image")),
        );
        agent
    }

    #[test]
    fn creation_emits_an_entity_and_copies_default_visibility() {
        let mut app = test_app();
        let resource = resource("builtin/echo");
        let agent = create_agent(&mut app, [resource.clone()].into_iter().collect());
        assert_eq!(
            app.world()
                .get_component::<AgentDefaultVisibility>(agent)
                .unwrap()
                .resources(),
            app.world()
                .get_component::<AgentDynamicVisibility>(agent)
                .unwrap()
                .resources()
        );
        assert_eq!(
            app.world()
                .get_component::<AgentContext>(agent)
                .unwrap()
                .system_prompt(),
            "system"
        );
    }

    #[test]
    fn resource_ids_are_used_as_tool_definition_names() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool_template(tool(first.clone(), "same"));
        app.register_tool_template(tool(second.clone(), "same"));
        let agent = create_agent(
            &mut app,
            [first.clone(), second.clone()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        );

        let available = match build_available_tools(app.world(), agent) {
            Ok(available) => available,
            Err(_) => panic!("available tool templates should be valid"),
        };
        assert_eq!(available.definitions.len(), 2);
        assert_eq!(available.definitions[0].name, first.to_string());
        assert_eq!(available.definitions[1].name, second.to_string());
    }

    #[test]
    fn unavailable_visible_resource_emits_agent_failure() {
        let mut app = test_app();
        let missing = ResourceId::parse("skill:local/missing").unwrap();
        let agent = create_agent(&mut app, [missing].into_iter().collect());
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
        });
        app.tick();
        app.tick();

        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.id, "turn-1");
        assert_eq!(failure.agent, agent);
        assert_eq!(failure.kind, AgentFailureKind::Agent);
        assert_eq!(
            failure.message,
            "ProviderMissing: resource provider was not registered"
        );
    }

    #[test]
    fn user_message_updates_context_and_sends_inference() {
        let mut app = test_app();
        let agent = create_agent(&mut app, BTreeSet::new());
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
        });
        app.tick();
        app.tick();

        let command = app
            .world()
            .event_reader::<InferenceRequest>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(command.id, "turn-1");
        assert_eq!(command.messages.len(), 2);
        assert!(command.tools.is_empty());
        assert_eq!(
            app.world()
                .get_component::<AgentContext>(agent)
                .unwrap()
                .messages(),
            [Message::User {
                content: "hello".into(),
                tool_calls: Vec::new(),
            }]
        );
    }

    #[test]
    fn tool_responses_continue_only_after_the_last_call() {
        let mut app = test_app();
        let first = resource("builtin/first");
        let second = resource("builtin/second");
        app.register_tool_template(tool(first.clone(), "first"));
        app.register_tool_template(tool(second.clone(), "second"));
        let agent = create_agent(
            &mut app,
            [first, second].into_iter().collect::<BTreeSet<_>>(),
        );
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::User {
                content: "use both".into(),
                tool_calls: vec![
                    ToolCall {
                        id: "first-call".into(),
                        resource: ResourceId::parse("tool:builtin/first").unwrap(),
                        arguments: "{}".into(),
                    },
                    ToolCall {
                        id: "second-call".into(),
                        resource: ResourceId::parse("tool:builtin/second").unwrap(),
                        arguments: "{}".into(),
                    },
                ],
            },
        });

        app.tick();
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Tool {
                tool_call_id: "first-call".into(),
                content: "first".into(),
            },
        });
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Tool {
                tool_call_id: "second-call".into(),
                content: "second".into(),
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut commands = 0;
        let mut settled_frames = 0;
        loop {
            app.tick();
            commands += app
                .world()
                .event_reader::<InferenceRequest>()
                .into_iter()
                .filter(|command| command.id == "turn-1")
                .count();
            let context = app.world().get_component::<AgentContext>(agent).unwrap();
            let status = app.world().get_component::<AgentStatus>(agent).unwrap();
            if commands == 1
                && status.pending_tools.is_empty()
                && context.messages().len() == 1
                && context.tool_context().len() == 2
            {
                settled_frames += 1;
                if settled_frames == 4 {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "tool responses did not settle");
            std::thread::yield_now();
        }
        assert_eq!(commands, 1);
    }

    #[test]
    fn loading_skills_are_templates_with_fresh_call_ids() {
        let resource = ResourceId::parse("skill:local/review").unwrap();
        let mut status = AgentStatus::default();
        assert!(status
            .load_skill(PendingToolCall {
                call: ToolCall {
                    id: "initial-call".into(),
                    resource: ResourceId::parse("skill:local/review").unwrap(),
                    arguments: "{}".into(),
                },
                resource,
                kind: ToolCallKind::Skill,
            })
            .is_ok());

        let first = expand_loading_skills(&status);
        let second = expand_loading_skills(&status);
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].call.id, "initial-call");
        assert_ne!(first[0].call.id, second[0].call.id);
        assert_eq!(first[0].resource, second[0].resource);
        assert!(status.unload_skill("skill:local/review:latest"));
        status.unload_all_skills();
    }
}

#[cfg(test)]
mod loading_skill_tests {
    use serde_json::json;
    use tool_plugin::{register_agent_tool, ToolPlugin, ToolTemplate};

    use super::*;

    fn test_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ToolPlugin::default())
            .add_plugin(AgentPlugin::default());
        let agent = app.world_mut().spawn();
        assert!(app
            .world_mut()
            .insert_component(agent, AgentStatus::default()));
        (app, agent)
    }

    #[test]
    fn assistant_history_uses_the_matching_inference_tool_schema() {
        let (mut app, agent) = test_app();
        let schema = vec![ToolDefinition {
            name: "tool0_read".into(),
            description: "Read a file.".into(),
            input_schema: json!({"type": "object"}),
        }];
        let message = AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Assistant {
                reasoning: None,
                content: Some("done".into()),
                tool_calls: Vec::new(),
            },
        };
        let events = app.world().event_sender();

        record_history_message(app.world_mut(), &message, &events, schema.clone());
        app.tick();

        let write = app
            .world()
            .event_reader::<AgentHistoryMessageWriteRequested>()
            .into_iter()
            .find(|event| event.id == "turn-1")
            .unwrap();
        assert_eq!(write.tool_schema, schema);
        assert!(app
            .world()
            .get_resource::<PendingInferenceToolSchemas>()
            .unwrap()
            .schemas
            .is_empty());
    }

    #[test]
    fn hidden_assistant_tool_call_becomes_a_denied_tool_message() {
        let (mut app, agent) = test_app();
        let resource_id = ResourceId::parse("shell:local/sh:latest").unwrap();
        attach_agent_tool_map(app.world_mut(), agent).unwrap();
        let map = register_agent_tool(
            app.world_mut(),
            agent,
            ResourceId::parse("tool:builtin/shell:latest").unwrap(),
            resource_id.clone(),
            ToolTemplate::new("ignored", "Run a shell command.", json!({"type": "object"}))
                .unwrap(),
        )
        .unwrap();
        assert!(app.world_mut().insert_component(
            agent,
            AgentDynamicVisibility {
                resources: BTreeSet::new(),
            },
        ));
        assert!(app.world_mut().insert_component(
            agent,
            AgentContext {
                system_prompt: "system".into(),
                messages: Vec::new(),
                tool_context: Vec::new(),
            },
        ));
        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .begin_turn("turn-1".into())
            .is_ok());
        let schema = vec![ToolDefinition {
            name: map.tool_name.clone(),
            description: "Run a shell command.".into(),
            input_schema: json!({"type": "object"}),
        }];
        app.world_mut()
            .get_resource_mut::<PendingInferenceToolSchemas>()
            .unwrap()
            .schemas
            .insert((agent, "turn-1".into()), schema);

        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Assistant {
                reasoning: None,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    tool_name: map.tool_name,
                    arguments: "{}".into(),
                }],
            },
        });
        app.tick();
        app.tick();

        assert!(app
            .world()
            .event_reader::<ToolCallEvent>()
            .into_iter()
            .next()
            .is_none());
        assert!(app
            .world()
            .get_component::<AgentContext>(agent)
            .unwrap()
            .tool_context()
            .iter()
            .any(|message| matches!(
                message,
                Message::Tool {
                    resource_id: actual,
                    tool_call_id,
                    content,
                } if actual == &resource_id
                    && tool_call_id == "call-1"
                    && content == TOOL_PERMISSION_DENIED
            )));
        assert!(app
            .world()
            .get_component::<AgentStatus>(agent)
            .unwrap()
            .loading_skills
            .is_empty());
    }

    #[test]
    fn load_and_unload_events_update_loading_skills() {
        let (mut app, agent) = test_app();
        let skill = ResourceId::parse("skill:local/review:latest").unwrap();
        for id in ["load-1", "load-2"] {
            app.world().send_event(LoadAgentSkill {
                id: id.into(),
                agent,
                resource_id: skill.clone(),
            });
        }
        app.tick();
        assert_eq!(
            app.world()
                .get_component::<AgentStatus>(agent)
                .unwrap()
                .loading_skills,
            BTreeSet::from([skill.clone()])
        );

        for id in ["unload-1", "unload-2"] {
            app.world().send_event(UnloadAgentSkill {
                id: id.into(),
                agent,
                resource_id: skill.clone(),
            });
        }
        app.tick();
        assert!(app
            .world()
            .get_component::<AgentStatus>(agent)
            .unwrap()
            .loading_skills
            .is_empty());
    }

    #[test]
    fn loading_non_skill_resource_emits_failure() {
        let (mut app, agent) = test_app();
        app.world().send_event(LoadAgentSkill {
            id: "load-1".into(),
            agent,
            resource_id: ResourceId::parse("tool:local/query:latest").unwrap(),
        });
        app.tick();
        app.tick();
        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.id, "load-1");
        assert_eq!(failure.kind, AgentFailureKind::Agent);
    }

    #[test]
    fn unload_all_event_clears_every_loading_skill() {
        let (mut app, agent) = test_app();
        for resource_id in [
            ResourceId::parse("skill:local/review:latest").unwrap(),
            ResourceId::parse("skill:local/commit:latest").unwrap(),
        ] {
            app.world().send_event(LoadAgentSkill {
                id: resource_id.to_string(),
                agent,
                resource_id,
            });
        }
        app.tick();
        assert_eq!(
            app.world()
                .get_component::<AgentStatus>(agent)
                .unwrap()
                .loading_skills
                .len(),
            2
        );

        app.world().send_event(UnloadAllAgentSkills {
            id: "unload-all-1".into(),
            agent,
        });
        app.tick();
        assert!(app
            .world()
            .get_component::<AgentStatus>(agent)
            .unwrap()
            .loading_skills
            .is_empty());
    }

    #[test]
    fn removing_a_resource_while_registration_is_in_flight_prevents_late_injection() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ToolPlugin::default())
            .add_plugin(AgentPlugin::default());
        let workspace = app.world_mut().spawn();
        let resource_id = ResourceId::parse("skill:local/review:latest").unwrap();
        app.world().send_event(AgentCreateRequest {
            id: "agent-create".into(),
            agent_id: ResourceId::parse("agent:demo/reviewer:latest").unwrap(),
            workspace_id: workspace,
            system_prompt: String::new(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            default_visibility: BTreeSet::from([resource_id.clone()]),
        });
        app.tick();
        app.tick();
        let agent = app
            .world()
            .event_reader::<AgentCreated>()
            .into_iter()
            .next()
            .unwrap()
            .agent;
        let registration = (0..8)
            .find_map(|_| {
                app.tick();
                app.world()
                    .event_reader::<AgentToolRegisterRequest>()
                    .into_iter()
                    .next()
                    .cloned()
            })
            .expect("default visibility should request tool registration");

        app.world().remove_agent_visible_resource(
            "remove-during-registration",
            agent,
            resource_id.clone(),
        );
        app.tick();
        register_agent_tool(
            app.world_mut(),
            agent,
            ResourceId::parse("tool:builtin/skill-loader:latest").unwrap(),
            resource_id.clone(),
            ToolTemplate::new("ignored", "test", json!({"type": "object"})).unwrap(),
        )
        .unwrap();
        app.world().send_event(AgentToolRegisterResponse {
            id: registration.id,
            agent,
            resource_id: resource_id.clone(),
            result: Ok(()),
        });
        app.tick();

        assert!(!app
            .world()
            .get_component::<AgentDynamicVisibility>(agent)
            .unwrap()
            .resources()
            .contains(&resource_id));
        assert!(app
            .world()
            .event_reader::<AgentVisibleResourceRemoved>()
            .into_iter()
            .any(|event| event.id == "remove-during-registration"));
        assert!(!app
            .world()
            .event_reader::<AgentVisibleResourceInjected>()
            .into_iter()
            .any(|event| event.resource_id == resource_id));
    }

    #[test]
    fn removing_skill_visibility_also_unloads_the_skill() {
        let (mut app, agent) = test_app();
        let skill = ResourceId::parse("skill:local/review:latest").unwrap();
        assert!(app.world_mut().insert_component(
            agent,
            AgentDefaultVisibility {
                resources: BTreeSet::from([skill.clone()]),
            },
        ));
        assert!(app.world_mut().insert_component(
            agent,
            AgentDynamicVisibility {
                resources: BTreeSet::from([skill.clone()]),
            },
        ));
        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .load_skill(skill.clone())
            .is_ok());

        app.world().send_event(SetAgentDefaultResourceVisibility {
            id: "remove-default-skill".into(),
            agent,
            resource_id: skill.clone(),
            visible: false,
        });
        app.tick();

        assert!(!app
            .world()
            .get_component::<AgentDynamicVisibility>(agent)
            .unwrap()
            .resources()
            .contains(&skill));
        assert!(!app
            .world()
            .get_component::<AgentStatus>(agent)
            .unwrap()
            .loading_skills
            .contains(&skill));
    }

    #[test]
    fn external_visibility_switch_rejects_non_default_resources() {
        let (mut app, agent) = test_app();
        assert!(app.world_mut().insert_component(
            agent,
            AgentDefaultVisibility {
                resources: BTreeSet::new(),
            },
        ));
        assert!(app.world_mut().insert_component(
            agent,
            AgentDynamicVisibility {
                resources: BTreeSet::new(),
            },
        ));
        app.world().send_event(SetAgentDefaultResourceVisibility {
            id: "inject-runtime-resource".into(),
            agent,
            resource_id: ResourceId::parse("tool:runtime/injected:latest").unwrap(),
            visible: true,
        });
        app.tick();
        app.tick();

        let failure = app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .find(|event| event.id == "inject-runtime-resource")
            .unwrap();
        assert_eq!(failure.kind, AgentFailureKind::Agent);
        assert!(app
            .world()
            .get_component::<AgentDynamicVisibility>(agent)
            .unwrap()
            .resources()
            .is_empty());
    }
}
