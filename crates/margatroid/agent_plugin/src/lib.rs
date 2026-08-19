use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use app_runtime_plugin::{RuntimeEventSender, RuntimeHandle, RuntimePlugin, WorldEventExt};
use core_plugin::{App, Component, Entity, Event, Plugin, Resource, World};
use inference_plugin::{
    CancelInferenceRequest, ContextCompactionInferenceRequest, ContextCompactionInferenceResponse,
    InferenceError, InferenceRequestEvent,
};
use margatroid_types::{
    AgentContextMessagesUpdated, AgentFailure, AgentFailureKind, AgentHistoryMessageWriteRequested,
    AgentMessage, Message, ResourceId, TokenUsage, ToolCall, ToolDefinition,
};
use mcl_plugin::{
    AgentMcl, AttachAgentMclRequest, MclBlockingInferenceRequest, MclCapabilityOwner,
    MclCommandValue, MclEffect, MclEffectsProduced, MclHistoryAppendRequested, MclMessage,
    MclPluginInstalled, MclProgram, MclResourceAliasDeclared, MclRuntimeMessage,
    WorkflowMclDetached, WorldMclExt,
};
use tool_plugin::set_agent_tool_alias;
use tool_plugin::{
    attach_agent_tool_map, AgentToolMap, AgentToolRegisterRequest, AgentToolRegisterResponse,
    CancelToolTurn, ToolCallEvent, ToolError, ToolErrorKind, ToolPluginInstalled,
    ToolTurnCompleted,
};

const TOOL_PERMISSION_DENIED: &str =
    "PermissionDenied: this resource is not available in the current tool schema; check the current tool schema before calling tools";
const CONTEXT_COMPACTION_PROMPT: &str = "You are acting as a context compaction engine. Summarize the conversation above into a concise checkpoint that allows another model to continue the work without losing essential context. Preserve current goals, decisions, constraints, exact identifiers, file paths, commands, errors, completed work, pending work, and the next required action. Do not mention this request, do not call tools, and output only the checkpoint text.";
const COMPACTED_SUMMARY_PREAMBLE: &str = "This checkpoint condenses an earlier span of the conversation. Treat it as established context and continue directly from the messages that follow.";

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbortAgentTurn {
    pub id: String,
    pub agent: Entity,
}

impl Event for AbortAgentTurn {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentContextCompactRequest {
    pub id: String,
    pub agent: Entity,
    pub retain_messages: usize,
}

impl Event for AgentContextCompactRequest {}

impl Plugin for AgentPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("AgentPlugin requires RuntimePlugin");
        }
        if !app.world().contains_resource::<ToolPluginInstalled>() {
            panic!("AgentPlugin requires ToolPlugin");
        }
        if !app.world().contains_resource::<MclPluginInstalled>() {
            panic!("AgentPlugin requires MclPlugin");
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
        app.world_mut()
            .insert_resource(PendingContextCompactions::default());
        app.world_mut()
            .insert_resource(PendingBlockingInferences::default());
        app.add_system(&self.schedule, agent_create_system)
            .add_system(&self.schedule, mcl_resource_alias_system)
            .add_system(&self.schedule, agent_visibility_change_system)
            .add_system(&self.schedule, mcl_history_append_system)
            .add_system(&self.schedule, mcl_effect_system)
            .add_system(&self.schedule, workflow_visibility_cleanup_system)
            .add_system(&self.schedule, collect_agent_tool_registration_system)
            .add_system(&self.schedule, cleanup_dead_agent_registrations_system)
            .add_system(&self.schedule, abort_agent_turn_system)
            .add_system(&self.schedule, context_compaction_system)
            .add_system(&self.schedule, blocking_inference_system)
            .add_system(&self.schedule, submit_agent_assistant_system)
            .add_system(&self.schedule, agent_message_system);
    }
}

fn mcl_resource_alias_system(world: &mut World) {
    let declarations = world
        .event_reader::<MclResourceAliasDeclared>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for declaration in declarations {
        if let Err(error) = set_agent_tool_alias(
            world,
            declaration.agent,
            declaration.resource_id.clone(),
            declaration.alias.clone(),
        ) {
            tracing::warn!(
                agent = ?declaration.agent,
                resource = %declaration.resource_id,
                alias = %declaration.alias,
                error = %error,
                "MCL resource alias could not be applied to AgentToolMap"
            );
        }
    }
}

fn mcl_history_append_system(world: &mut World) {
    let requests = world
        .event_reader::<MclHistoryAppendRequested>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for request in requests {
        let tool_schema = if matches!(request.message.message, Message::Assistant { .. }) {
            world
                .get_resource::<PendingInferenceToolSchemas>()
                .and_then(|pending| pending.schemas.get(&(request.agent, request.id.clone())))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        record_history_message(
            world,
            &AgentMessage {
                id: request.id,
                agent: request.agent,
                message: request.message.message,
                usage: request.message.usage,
            },
            &events,
            tool_schema,
        );
    }
}

#[derive(Clone, Debug)]
pub struct AgentCreateRequest {
    pub id: String,
    pub agent_id: ResourceId,
    pub workspace_id: Entity,
    pub base_mcl: std::sync::Arc<MclProgram>,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tool_context: Vec<Message>,
    pub ordered_messages: Vec<Message>,
    pub token_usage: TokenUsage,
    pub last_input_tokens: u64,
    pub context_window_tokens: u64,
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
pub struct SubmitAgentAssistant {
    pub id: String,
    pub agent: Entity,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<SubmitAgentAssistantToolCall>,
}

impl Event for SubmitAgentAssistant {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitAgentAssistantToolCall {
    pub id: String,
    pub resource_id: ResourceId,
    pub arguments: String,
}

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
    MclSetupFailed,
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
    fn agent_is_working(&self, agent: Entity) -> Option<bool>;
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

    fn agent_is_working(&self, agent: Entity) -> Option<bool> {
        self.get_component::<AgentStatus>(agent)
            .map(AgentStatus::is_working)
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
    ordered_messages: Vec<Message>,
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

    pub fn ordered_messages(&self) -> &[Message] {
        &self.ordered_messages
    }

    pub fn append_message(&mut self, agent: Entity, message: Message, events: &RuntimeEventSender) {
        assert_conversation_message(&message);
        self.messages.push(message.clone());
        self.ordered_messages.push(message);
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
        self.tool_context.clear();
        self.ordered_messages = self.messages.clone();
        self.notify_updated(agent, events);
    }

    pub fn append_tool_context(
        &mut self,
        agent: Entity,
        message: Message,
        events: &RuntimeEventSender,
    ) {
        assert!(matches!(message, Message::Tool { .. }));
        self.tool_context.push(message.clone());
        self.ordered_messages.push(message);
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
            ordered_messages: self.ordered_messages.clone(),
        });
    }
}

impl Component for AgentContext {}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTokenUsage {
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_hit_tokens: u64,
    cache_hit_rate: f64,
    last_input_tokens: u64,
    context_window_tokens: u64,
}

impl AgentTokenUsage {
    fn from_totals(usage: &TokenUsage, last_input_tokens: u64, context_window_tokens: u64) -> Self {
        let mut totals = Self {
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage.output_tokens,
            total_cache_hit_tokens: usage.cache_hit_tokens,
            cache_hit_rate: 0.0,
            last_input_tokens,
            context_window_tokens,
        };
        totals.recalculate_cache_hit_rate();
        totals
    }

    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }

    pub fn total_cache_hit_tokens(&self) -> u64 {
        self.total_cache_hit_tokens
    }

    pub fn cache_hit_rate(&self) -> f64 {
        self.cache_hit_rate
    }

    pub fn last_input_tokens(&self) -> u64 {
        self.last_input_tokens
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens
    }

    pub(crate) fn add(&mut self, usage: &TokenUsage) {
        self.last_input_tokens = usage.input_tokens;
        self.total_input_tokens = self.total_input_tokens.saturating_add(usage.input_tokens);
        self.total_output_tokens = self.total_output_tokens.saturating_add(usage.output_tokens);
        self.total_cache_hit_tokens = self
            .total_cache_hit_tokens
            .saturating_add(usage.cache_hit_tokens);
        self.recalculate_cache_hit_rate();
    }

    fn recalculate_cache_hit_rate(&mut self) {
        self.cache_hit_rate = if self.total_input_tokens == 0 {
            0.0
        } else {
            self.total_cache_hit_tokens as f64 / self.total_input_tokens as f64
        };
    }
}

impl Component for AgentTokenUsage {}

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
    owners: BTreeSet<MclCapabilityOwner>,
}

#[derive(Default)]
struct PendingInferenceToolSchemas {
    schemas: HashMap<(Entity, String), Vec<ToolDefinition>>,
}

impl Resource for PendingInferenceToolSchemas {}

#[derive(Default)]
struct PendingContextCompactions {
    requests: HashMap<(Entity, String), PendingContextCompaction>,
}

impl Resource for PendingContextCompactions {}

#[derive(Default)]
struct PendingBlockingInferences {
    requests: HashMap<
        (Entity, String),
        std::sync::mpsc::Sender<Result<MclCommandValue, mcl_plugin::MclError>>,
    >,
}

impl Resource for PendingBlockingInferences {}

struct PendingContextCompaction {
    original_messages: Vec<Message>,
    retained_messages: Vec<Message>,
}

#[derive(Default)]
pub(crate) struct AgentStatus {
    turn_id: Option<String>,
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
    pub(crate) fn abort_turn(&mut self) -> Option<String> {
        self.turn_id.take()
    }
    pub(crate) fn is_working(&self) -> bool {
        self.turn_id.is_some()
    }
}

impl Component for AgentStatus {}

struct AvailableTools {
    definitions: Vec<ToolDefinition>,
}

enum ConversationTurnResult {
    WaitForTools,
    RequestInference,
}

enum AgentStepError {
    AgentMissing,
    IdentityMissing,
    ContextMissing,
    StatusMissing,
    TokenUsageMissing,
    ToolMapMissing,
    InvalidMessage,
    InvalidToolBatch,
    ContextNotCompactable,
    ContextChanged,
    InvalidCompactionResponse,
    Inference(InferenceError),
    Tool(ToolError),
}

impl AgentStepError {
    fn failure_message(&self) -> String {
        match self {
            Self::AgentMissing => "AgentMissing: agent entity is not alive".into(),
            Self::IdentityMissing => "IdentityMissing: agent identity is missing".into(),
            Self::ContextMissing => "ContextMissing: agent context is missing".into(),
            Self::StatusMissing => "StatusMissing: agent status is missing".into(),
            Self::TokenUsageMissing => "TokenUsageMissing: agent token usage is missing".into(),
            Self::ToolMapMissing => "ToolMapMissing: Agent tool map is missing".into(),
            Self::InvalidMessage => "InvalidMessage: message type is invalid".into(),
            Self::InvalidToolBatch => "InvalidToolBatch: tool call batch is invalid".into(),
            Self::ContextNotCompactable => {
                "ContextNotCompactable: Agent context cannot be compacted".into()
            }
            Self::ContextChanged => {
                "ContextChanged: Agent context changed during compaction".into()
            }
            Self::InvalidCompactionResponse => {
                "InvalidCompactionResponse: context compaction response does not match a pending request".into()
            }
            Self::Inference(error) => error.to_string(),
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
            ordered_messages: if request.ordered_messages.is_empty() {
                request
                    .messages
                    .iter()
                    .chain(request.tool_context.iter())
                    .cloned()
                    .collect()
            } else {
                request.ordered_messages.clone()
            },
        },
    ) && world.insert_component(
        agent,
        AgentTokenUsage::from_totals(
            &request.token_usage,
            request.last_input_tokens,
            request.context_window_tokens,
        ),
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
    let restored_messages = if request.ordered_messages.is_empty() {
        request
            .messages
            .iter()
            .chain(request.tool_context.iter())
            .cloned()
            .collect()
    } else {
        request.ordered_messages.clone()
    };
    let initial_effects = match world.attach_agent_mcl(
        agent,
        AttachAgentMclRequest {
            base: std::sync::Arc::clone(&request.base_mcl),
            system_prompt: request.system_prompt.clone(),
            context_window_tokens: request.context_window_tokens,
            restored_messages,
            default_visibility: request.default_visibility.clone(),
        },
    ) {
        Ok(effects) => effects,
        Err(error) => {
            world.despawn(agent);
            return Err(AgentCreateError::new(
                AgentCreateErrorKind::MclSetupFailed,
                error.to_string(),
            ));
        }
    };
    if !initial_effects.is_empty() {
        world.send_event(MclEffectsProduced {
            id: request.id.clone(),
            agent,
            effects: initial_effects,
        });
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
        inject_visible_resource(
            world,
            &event.id,
            event.agent,
            &event.resource_id,
            MclCapabilityOwner::External("manual".into()),
            &events,
        );
    }
    for event in removes {
        if let Err(error) =
            world.revoke_agent_resource(event.agent, &MclCapabilityOwner::Base, &event.resource_id)
        {
            report_visibility_operation_failure(
                &event.id,
                event.agent,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    error.to_string(),
                ),
                &events,
            );
            continue;
        }
        if let Some(registration) = world
            .get_resource_mut::<InFlightVisibilityRegistrations>()
            .expect("AgentPlugin is not installed")
            .registrations
            .get_mut(&(event.agent, event.resource_id.clone()))
        {
            registration.owners.remove(&MclCapabilityOwner::Base);
        }
        if let Err(error) = remove_visible_resource(
            world,
            &event.id,
            event.agent,
            &event.resource_id,
            &MclCapabilityOwner::External("manual".into()),
            &events,
        ) {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
    for event in default_changes {
        let is_default = world
            .get_component::<AgentMcl>(event.agent)
            .is_some_and(|mcl| {
                mcl.capabilities()
                    .default_resources()
                    .contains(&event.resource_id)
            });
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
            inject_visible_resource(
                world,
                &event.id,
                event.agent,
                &event.resource_id,
                MclCapabilityOwner::Base,
                &events,
            );
        } else if let Err(error) = remove_visible_resource(
            world,
            &event.id,
            event.agent,
            &event.resource_id,
            &MclCapabilityOwner::Base,
            &events,
        ) {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
    for event in restores {
        let resources = world.get_component::<AgentMcl>(event.agent).map(|mcl| {
            mcl.capabilities()
                .default_resources()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        });
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
        if let Err(error) = remove_all_visible_resources(world, &event.id, event.agent, &events) {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
            continue;
        }
        for resource_id in resources {
            inject_visible_resource(
                world,
                &event.id,
                event.agent,
                &resource_id,
                MclCapabilityOwner::Base,
                &events,
            );
        }
    }
    for event in remove_all {
        if let Err(error) = remove_all_visible_resources(world, &event.id, event.agent, &events) {
            report_visibility_operation_failure(&event.id, event.agent, error, &events);
        }
    }
}

fn mcl_effect_system(world: &mut World) {
    let produced = world
        .event_reader::<MclEffectsProduced>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for produced in produced {
        for effect in produced.effects {
            let result = match effect {
                MclEffect::ResolveResources { owner, resources } => {
                    for resource_id in resources {
                        inject_visible_resource(
                            world,
                            &produced.id,
                            produced.agent,
                            &resource_id,
                            owner.clone(),
                            &events,
                        );
                    }
                    Ok(())
                }
                MclEffect::RequestInference { request } => {
                    send_inference_request(world, &produced.id, produced.agent, &request, &events)
                }
                MclEffect::ExecuteTools { calls } => {
                    let schema = take_pending_tool_schema(world, produced.agent, &produced.id);
                    dispatch_assistant_tool_calls(
                        world,
                        &produced.id,
                        produced.agent,
                        &calls,
                        &schema,
                        &events,
                    )
                    .map(|_| ())
                }
                MclEffect::FinishTurn => {
                    take_pending_tool_schema(world, produced.agent, &produced.id);
                    world
                        .get_component_mut::<AgentStatus>(produced.agent)
                        .ok_or(AgentStepError::StatusMissing)
                        .and_then(|status| status.finish_turn(&produced.id))
                }
            };
            if let Err(error) = result {
                events.send_event(AgentFailure {
                    id: produced.id.clone(),
                    agent: produced.agent,
                    kind: AgentFailureKind::Agent,
                    message: error.failure_message(),
                });
            }
        }
    }
}

fn workflow_visibility_cleanup_system(world: &mut World) {
    let detached = world
        .event_reader::<WorkflowMclDetached>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for event in detached {
        let owner = MclCapabilityOwner::Workflow(event.instance_id);
        for registration in world
            .get_resource_mut::<InFlightVisibilityRegistrations>()
            .expect("AgentPlugin is installed")
            .registrations
            .values_mut()
            .filter(|registration| registration.agent == event.agent)
        {
            registration.owners.remove(&owner);
        }
        for resource_id in event.removed_resources {
            events.send_event(AgentVisibleResourceRemoved {
                id: event.id.clone(),
                agent: event.agent,
                resource_id,
            });
        }
    }
}

fn inject_visible_resource(
    world: &mut World,
    id: &str,
    agent: Entity,
    resource_id: &ResourceId,
    owner: MclCapabilityOwner,
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
    let Some(mcl) = world.get_component::<AgentMcl>(agent) else {
        send_visibility_injection_failed(
            id,
            agent,
            resource_id,
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                "Agent MCL capability store is missing",
            ),
            events,
        );
        return;
    };
    if mcl.capabilities().is_visible(resource_id) {
        if let Err(error) = world.grant_agent_resource(agent, owner, resource_id.clone()) {
            send_visibility_injection_failed(
                id,
                agent,
                resource_id,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    error.to_string(),
                ),
                events,
            );
            return;
        }
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
        if let Err(error) = world.grant_agent_resource(agent, owner, resource_id.clone()) {
            send_visibility_injection_failed(
                id,
                agent,
                resource_id,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    error.to_string(),
                ),
                events,
            );
            return;
        }
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
        registration.owners.insert(owner);
        return;
    }
    static NEXT_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);
    let registration_id = format!(
        "agent-tool-registration-{}",
        NEXT_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut notification_ids = BTreeSet::new();
    notification_ids.insert(id.to_owned());
    let mut owners = BTreeSet::new();
    owners.insert(owner);
    registrations.registrations.insert(
        key,
        InFlightVisibilityRegistration {
            registration_id: registration_id.clone(),
            agent,
            resource_id: resource_id.clone(),
            notification_ids,
            owners,
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
    owner: &MclCapabilityOwner,
    events: &RuntimeEventSender,
) -> Result<(), AgentVisibilityError> {
    if !world.is_alive(agent) {
        return Err(AgentVisibilityError::new(
            AgentVisibilityErrorKind::AgentMissing,
            "Agent entity is not alive",
        ));
    }
    world
        .revoke_agent_resource(agent, owner, resource_id)
        .map_err(|error| {
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                error.to_string(),
            )
        })?;
    if let Some(registration) = world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed")
        .registrations
        .get_mut(&(agent, resource_id.clone()))
    {
        registration.owners.remove(owner);
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
    events: &RuntimeEventSender,
) -> Result<(), AgentVisibilityError> {
    if !world.is_alive(agent) {
        return Err(AgentVisibilityError::new(
            AgentVisibilityErrorKind::AgentMissing,
            "Agent entity is not alive",
        ));
    }
    let previous = world
        .get_component::<AgentMcl>(agent)
        .ok_or_else(|| {
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                "Agent MCL capability store is missing",
            )
        })?
        .capabilities()
        .visible_resources()
        .cloned()
        .collect::<BTreeSet<_>>();
    for owner in [
        MclCapabilityOwner::Base,
        MclCapabilityOwner::External("manual".into()),
    ] {
        world
            .clear_agent_resource_owner(agent, &owner)
            .map_err(|error| {
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    error.to_string(),
                )
            })?;
    }
    for registration in world
        .get_resource_mut::<InFlightVisibilityRegistrations>()
        .expect("AgentPlugin is not installed")
        .registrations
        .values_mut()
        .filter(|registration| registration.agent == agent)
    {
        registration.owners.remove(&MclCapabilityOwner::Base);
        registration
            .owners
            .remove(&MclCapabilityOwner::External("manual".into()));
    }
    let effective = world
        .get_component::<AgentMcl>(agent)
        .ok_or_else(|| {
            AgentVisibilityError::new(
                AgentVisibilityErrorKind::VisibilityMissing,
                "Agent MCL capability store is missing",
            )
        })?
        .capabilities()
        .visible_resources()
        .cloned()
        .collect::<BTreeSet<_>>();
    for resource_id in previous.difference(&effective).cloned() {
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
        let owners = registration
            .owners
            .iter()
            .filter(|owner| capability_owner_is_active(world, registration.agent, owner))
            .cloned()
            .collect::<Vec<_>>();
        if owners.is_empty() {
            continue;
        }
        let mut grant_failed = None;
        for owner in owners {
            if let Err(error) = world.grant_agent_resource(
                registration.agent,
                owner,
                registration.resource_id.clone(),
            ) {
                grant_failed = Some(error);
                break;
            }
        }
        if let Some(error) = grant_failed {
            notify_registration_failure(
                &registration,
                AgentVisibilityError::new(
                    AgentVisibilityErrorKind::VisibilityMissing,
                    error.to_string(),
                ),
                &events,
            );
            continue;
        }
        for id in &registration.notification_ids {
            events.send_event(AgentVisibleResourceInjected {
                id: id.clone(),
                agent: registration.agent,
                resource_id: registration.resource_id.clone(),
            });
        }
    }
}

fn capability_owner_is_active(world: &World, agent: Entity, owner: &MclCapabilityOwner) -> bool {
    match owner {
        MclCapabilityOwner::Base | MclCapabilityOwner::External(_) => true,
        MclCapabilityOwner::Workflow(instance_id) => world
            .get_component::<AgentMcl>(agent)
            .is_some_and(|mcl| mcl.workflows().any(|(id, _)| id == instance_id)),
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
    let dead_compactions = world
        .get_resource::<PendingContextCompactions>()
        .expect("AgentPlugin is not installed")
        .requests
        .keys()
        .filter(|(agent, _)| !world.is_alive(*agent))
        .cloned()
        .collect::<Vec<_>>();
    let compactions = world
        .get_resource_mut::<PendingContextCompactions>()
        .expect("AgentPlugin is not installed");
    for key in dead_compactions {
        compactions.requests.remove(&key);
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
        // The Base Driver is the owner of message routing. Keep this mailbox
        // ingress separate from history/status bookkeeping below.
        world.send_event(MclRuntimeMessage {
            id: message.id.clone(),
            agent,
            message: MclMessage::new(message.message.clone(), message.usage.clone()),
        });
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

fn submit_agent_assistant_system(world: &mut World) {
    let requests = world
        .event_reader::<SubmitAgentAssistant>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for request in requests {
        let result = (|| {
            if request.tool_calls.is_empty() {
                return Err(AgentStepError::InvalidToolBatch);
            }
            world
                .get_component_mut::<AgentStatus>(request.agent)
                .ok_or(AgentStepError::StatusMissing)?
                .begin_turn(request.id.clone())?;
            let maps = world
                .get_component::<AgentToolMap>(request.agent)
                .ok_or(AgentStepError::ToolMapMissing)?;
            let visibility = world
                .get_component::<AgentMcl>(request.agent)
                .ok_or(AgentStepError::ContextMissing)?;
            let mut tool_calls = Vec::with_capacity(request.tool_calls.len());
            let mut tool_schema = Vec::with_capacity(request.tool_calls.len());
            for call in &request.tool_calls {
                if call.resource_id.resource_type() != "skill"
                    || !visibility.capabilities().is_visible(&call.resource_id)
                {
                    return Err(AgentStepError::InvalidToolBatch);
                }
                let matched = maps.get_by_resource(&call.resource_id);
                if matched.len() != 1 {
                    return Err(AgentStepError::InvalidToolBatch);
                }
                let map = matched[0];
                tool_calls.push(ToolCall {
                    id: call.id.clone(),
                    tool_name: map.tool_name.clone(),
                    arguments: call.arguments.clone(),
                });
                tool_schema.push(ToolDefinition {
                    name: map.tool_name.clone(),
                    description: map.template.description.clone(),
                    input_schema: map.template.parameters.clone(),
                });
            }
            world
                .get_resource_mut::<PendingInferenceToolSchemas>()
                .expect("AgentPlugin is installed")
                .schemas
                .insert((request.agent, request.id.clone()), tool_schema);
            events.send_event(AgentMessage {
                id: request.id.clone(),
                agent: request.agent,
                message: Message::Assistant {
                    reasoning: request.reasoning.clone(),
                    content: request.content.clone(),
                    tool_calls,
                },
                usage: None,
            });
            Ok(())
        })();
        if let Err(error) = result {
            if world
                .get_component::<AgentStatus>(request.agent)
                .is_some_and(|status| status.turn_id.as_deref() == Some(&request.id))
            {
                let _ = world
                    .get_component_mut::<AgentStatus>(request.agent)
                    .and_then(AgentStatus::abort_turn);
            }
            events.send_event(AgentFailure {
                id: request.id,
                agent: request.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }
}

fn abort_agent_turn_system(world: &mut World) {
    let requests = world
        .event_reader::<AbortAgentTurn>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();
    for request in requests {
        let turn_id = world
            .get_component_mut::<AgentStatus>(request.agent)
            .and_then(AgentStatus::abort_turn);
        let Some(turn_id) = turn_id else {
            tracing::warn!(request_id = %request.id, ?request.agent, "Agent turn abort ignored because Agent is idle");
            continue;
        };
        if let Err(error) = clear_tool_context(world, request.agent, &events) {
            tracing::warn!(request_id = %request.id, ?request.agent, error = %error.failure_message(), "Agent tool context could not be cleared after abort");
        }
        world
            .get_resource_mut::<PendingInferenceToolSchemas>()
            .expect("AgentPlugin is installed")
            .schemas
            .remove(&(request.agent, turn_id.clone()));
        world
            .get_resource_mut::<PendingContextCompactions>()
            .expect("AgentPlugin is installed")
            .requests
            .remove(&(request.agent, turn_id.clone()));
        events.send_event(CancelInferenceRequest {
            id: turn_id.clone(),
            agent: request.agent,
        });
        events.send_event(CancelToolTurn {
            turn_id: turn_id.clone(),
            agent: request.agent,
        });
        tracing::info!(request_id = %request.id, turn_id, ?request.agent, "Agent turn aborted");
    }
}

fn context_compaction_system(world: &mut World) {
    let requests = world
        .event_reader::<AgentContextCompactRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let responses = world
        .event_reader::<ContextCompactionInferenceResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for request in requests {
        if let Err(error) = begin_context_compaction(world, &request, &events) {
            events.send_event(AgentFailure {
                id: request.id,
                agent: request.agent,
                kind: AgentFailureKind::Agent,
                message: error.failure_message(),
            });
        }
    }
    for response in responses {
        if !world
            .get_resource::<PendingContextCompactions>()
            .expect("AgentPlugin is installed")
            .requests
            .contains_key(&(response.agent, response.id.clone()))
        {
            continue;
        }
        if let Err(error) = complete_context_compaction(world, &response, &events) {
            let kind = if matches!(error, AgentStepError::Inference(_)) {
                AgentFailureKind::Inference
            } else {
                AgentFailureKind::Agent
            };
            events.send_event(AgentFailure {
                id: response.id,
                agent: response.agent,
                kind,
                message: error.failure_message(),
            });
        }
    }
}

fn blocking_inference_system(world: &mut World) {
    let requests = world
        .event_reader::<MclBlockingInferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let responses = world
        .event_reader::<ContextCompactionInferenceResponse>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let events = world.event_sender();

    for request in requests {
        let snapshot = match world.assemble_model_request(request.agent, &request.request) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = request.reply.send(Err(error));
                continue;
            }
        };
        let agent_id = match world.get_component::<AgentIdentity>(request.agent) {
            Some(identity) => identity.id().clone(),
            None => {
                let _ = request.reply.send(Err(mcl_plugin::MclError::new(
                    mcl_plugin::MclErrorKind::AgentMissing,
                    "Agent identity is missing",
                )));
                continue;
            }
        };
        let mut messages = Vec::with_capacity(snapshot.messages.len() + 1);
        messages.push(Message::System {
            content: snapshot.system,
        });
        messages.extend(snapshot.messages);
        world
            .get_resource_mut::<PendingBlockingInferences>()
            .expect("AgentPlugin is installed")
            .requests
            .insert((request.agent, request.id.clone()), request.reply);
        events.send_event(ContextCompactionInferenceRequest {
            id: request.id,
            agent: request.agent,
            agent_id,
            messages,
        });
    }

    for response in responses {
        let reply = world
            .get_resource_mut::<PendingBlockingInferences>()
            .expect("AgentPlugin is installed")
            .requests
            .remove(&(response.agent, response.id.clone()));
        let Some(reply) = reply else { continue };
        let result = response
            .result
            .map(|summary| MclCommandValue::Json(serde_json::Value::String(summary)))
            .map_err(|error| {
                mcl_plugin::MclError::new(mcl_plugin::MclErrorKind::TypeMismatch, error.to_string())
            });
        let _ = reply.send(result);
    }
}

fn begin_context_compaction(
    world: &mut World,
    request: &AgentContextCompactRequest,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    if request.id.is_empty() || !world.is_alive(request.agent) {
        return Err(AgentStepError::AgentMissing);
    }
    let context = world
        .get_component::<AgentContext>(request.agent)
        .ok_or(AgentStepError::ContextMissing)?;
    if !context.tool_context.is_empty() || context.messages.len() <= request.retain_messages {
        return Err(AgentStepError::ContextNotCompactable);
    }
    let original_messages = context.messages.clone();
    let compacted_count = original_messages.len() - request.retain_messages;
    let compacting_messages = original_messages[..compacted_count].to_vec();
    let retained_messages = original_messages[compacted_count..].to_vec();
    let system_prompt = context.system_prompt.clone();
    let agent_id = world
        .get_component::<AgentIdentity>(request.agent)
        .map(|identity| identity.id().clone())
        .ok_or(AgentStepError::IdentityMissing)?;

    world
        .get_component_mut::<AgentStatus>(request.agent)
        .ok_or(AgentStepError::StatusMissing)?
        .begin_turn(request.id.clone())?;
    world
        .get_resource_mut::<PendingContextCompactions>()
        .expect("AgentPlugin is installed")
        .requests
        .insert(
            (request.agent, request.id.clone()),
            PendingContextCompaction {
                original_messages,
                retained_messages,
            },
        );

    let mut messages = Vec::with_capacity(compacting_messages.len() + 2);
    messages.push(Message::System {
        content: system_prompt,
    });
    messages.extend(compacting_messages);
    messages.push(Message::User {
        content: CONTEXT_COMPACTION_PROMPT.into(),
    });
    events.send_event(ContextCompactionInferenceRequest {
        id: request.id.clone(),
        agent: request.agent,
        agent_id,
        messages,
    });
    Ok(())
}

fn complete_context_compaction(
    world: &mut World,
    response: &ContextCompactionInferenceResponse,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let pending = world
        .get_resource_mut::<PendingContextCompactions>()
        .expect("AgentPlugin is installed")
        .requests
        .remove(&(response.agent, response.id.clone()))
        .ok_or(AgentStepError::InvalidCompactionResponse)?;
    let current_turn_matches = world
        .get_component::<AgentStatus>(response.agent)
        .ok_or(AgentStepError::StatusMissing)?
        .turn_id
        .as_deref()
        == Some(&response.id);
    if !current_turn_matches {
        return Err(AgentStepError::InvalidCompactionResponse);
    }

    let summary = match &response.result {
        Ok(summary) if !summary.trim().is_empty() => summary.trim(),
        Ok(_) => {
            world
                .get_component_mut::<AgentStatus>(response.agent)
                .expect("AgentStatus existence was checked")
                .finish_turn(&response.id)?;
            return Err(AgentStepError::InvalidCompactionResponse);
        }
        Err(error) => {
            world
                .get_component_mut::<AgentStatus>(response.agent)
                .expect("AgentStatus existence was checked")
                .finish_turn(&response.id)?;
            return Err(AgentStepError::Inference(error.clone()));
        }
    };
    let context_unchanged = world
        .get_component::<AgentContext>(response.agent)
        .ok_or(AgentStepError::ContextMissing)
        .map(|context| {
            context.messages == pending.original_messages && context.tool_context.is_empty()
        })?;
    if !context_unchanged {
        world
            .get_component_mut::<AgentStatus>(response.agent)
            .expect("AgentStatus existence was checked")
            .finish_turn(&response.id)?;
        return Err(AgentStepError::ContextChanged);
    }

    let mut messages = Vec::with_capacity(pending.retained_messages.len() + 1);
    messages.push(Message::User {
        content: format!(
            "{COMPACTED_SUMMARY_PREAMBLE}\n\n<compacted-summary>\n{summary}\n</compacted-summary>"
        ),
    });
    messages.extend(pending.retained_messages);
    world
        .get_component_mut::<AgentContext>(response.agent)
        .ok_or(AgentStepError::ContextMissing)?
        .rewrite_messages(response.agent, messages, events);
    world
        .get_component_mut::<AgentStatus>(response.agent)
        .ok_or(AgentStepError::StatusMissing)?
        .finish_turn(&response.id)?;
    Ok(())
}

fn handle_agent_message(
    world: &mut World,
    event: &AgentMessage,
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let agent = event.agent;
    match &event.message {
        Message::System { .. } => return Err(AgentStepError::InvalidMessage),
        Message::User { .. } => {
            world
                .get_component_mut::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .begin_turn(event.id.clone())?;
            clear_tool_context(world, agent, events)?;
            append_conversation_message(world, agent, event.message.clone(), events)?;
        }
        Message::Assistant { .. } => {
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
            world
                .get_resource_mut::<PendingInferenceToolSchemas>()
                .expect("AgentPlugin is installed")
                .schemas
                .insert((agent, event.id.clone()), tool_schema.clone());
            if let Some(usage) = &event.usage {
                world
                    .get_component_mut::<AgentTokenUsage>(agent)
                    .ok_or(AgentStepError::TokenUsageMissing)?
                    .add(usage);
            }
            append_conversation_message(world, agent, event.message.clone(), events)?;
        }
        Message::Tool { .. } => {
            if world
                .get_component::<AgentStatus>(agent)
                .ok_or(AgentStepError::StatusMissing)?
                .turn_id
                .as_deref()
                != Some(&event.id)
            {
                return Err(AgentStepError::InvalidToolBatch);
            }
            append_tool_context(world, agent, event.message.clone(), events)?;
        }
    }
    // Message routing and the next action belong to base.lua. This function
    // only maintains Agent bookkeeping before the message reaches the Driver.
    Ok(ConversationTurnResult::WaitForTools)
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
        } if resource_id.resource_type() == "skill" => Message::Tool {
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
        usage: if matches!(event.message, Message::Assistant { .. }) {
            event.usage.clone()
        } else {
            None
        },
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
    events: &RuntimeEventSender,
) -> Result<ConversationTurnResult, AgentStepError> {
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    let calls = tool_calls.to_vec();
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
        .get_component::<AgentMcl>(agent)
        .ok_or(AgentStepError::ContextMissing)?;
    let schema_names = tool_schema
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut call_ids = BTreeSet::new();
    let mut authorized = Vec::new();
    let mut denied = Vec::new();
    for call in tool_calls {
        if call.id.is_empty() || call.tool_name.is_empty() || !call_ids.insert(call.id.clone()) {
            return Err(AgentStepError::InvalidToolBatch);
        }
        let map = maps
            .get_by_name(&call.tool_name)
            .ok_or(AgentStepError::InvalidToolBatch)?;
        if !schema_names.contains(call.tool_name.as_str())
            || !visibility.capabilities().is_visible(&map.resource_id)
        {
            denied.push((call.id.clone(), map.resource_id.clone()));
        } else {
            authorized.push(call.clone());
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
            usage: None,
        });
    }
    let result = dispatch_tool_calls(world, id, agent, &authorized, events)?;
    if !denied_only {
        return Ok(result);
    }
    events.send_event(ToolTurnCompleted {
        turn_id: id.to_owned(),
        agent,
    });
    Ok(ConversationTurnResult::WaitForTools)
}

fn send_inference_request(
    world: &mut World,
    id: &str,
    agent: Entity,
    request: &str,
    events: &RuntimeEventSender,
) -> Result<(), AgentStepError> {
    let snapshot = world
        .assemble_model_request(agent, request)
        .map_err(|_| AgentStepError::ContextMissing)?;
    let available_tools = build_available_tools(world, agent, &snapshot.visible_resources)?;
    let mut messages = Vec::with_capacity(snapshot.messages.len() + 1);
    messages.push(Message::System {
        content: snapshot.system,
    });
    messages.extend(snapshot.messages);
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

fn build_available_tools(
    world: &World,
    agent: Entity,
    resources: &[ResourceId],
) -> Result<AvailableTools, AgentStepError> {
    let maps = world
        .get_component::<AgentToolMap>(agent)
        .ok_or(AgentStepError::ToolMapMissing)?;
    let mut definitions = Vec::with_capacity(resources.len());
    for resource in resources {
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tool_plugin::{register_agent_tool, ToolPlugin, ToolTemplate};

    use super::*;

    fn test_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ToolPlugin::default())
            .add_plugin(mcl_plugin::MclPlugin::open(std::env::temp_dir()).unwrap())
            .add_plugin(AgentPlugin::default());
        let agent = app.world_mut().spawn();
        assert!(app
            .world_mut()
            .insert_component(agent, AgentStatus::default()));
        (app, agent)
    }

    fn base_mcl() -> std::sync::Arc<mcl_plugin::MclProgram> {
        mcl_plugin::compile_mcl(mcl_plugin::MclCompileRequest {
            root: mcl_plugin::MclSource::new(
                ResourceId::parse("mcl:local/test:latest").unwrap(),
                r#"base context test {
block conversation: context persistent;
view messages: messages { select entry from conversation; }
view tools: tools { select resource from capabilities.dynamic; }
request inference { system = agent.system; messages = messages; tools = tools; }
on agent.created { restore capabilities.dynamic from capabilities.default; }
on message.user { append event.entry into conversation; emit inference using inference; }
on message.assistant where event.tool_calls is not empty { append event.exchange into conversation; emit tools event.tool_calls; }
on message.tool { append event.entry into event.exchange; }
on tool.batch.completed { emit inference using inference; }
on message.assistant where event.tool_calls is empty { append event.entry into conversation; finish turn; }
}"#,
                std::path::PathBuf::from("/test/main.mcl"),
            ),
            dependencies: std::collections::BTreeMap::new(),
        })
        .unwrap()
    }

    #[test]
    fn work_status_tracks_the_active_turn() {
        let (mut app, agent) = test_app();
        assert_eq!(app.world().agent_is_working(agent), Some(false));

        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .begin_turn("turn-1".into())
            .is_ok());
        assert_eq!(app.world().agent_is_working(agent), Some(true));

        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .finish_turn("turn-1")
            .is_ok());
        assert_eq!(app.world().agent_is_working(agent), Some(false));
    }

    #[test]
    fn token_usage_accumulates_and_recalculates_cache_hit_rate() {
        let mut totals = AgentTokenUsage::from_totals(
            &TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_hit_tokens: 25,
            },
            100,
            1_000,
        );
        totals.add(&TokenUsage {
            input_tokens: 300,
            output_tokens: 80,
            cache_hit_tokens: 175,
        });

        assert_eq!(totals.total_input_tokens(), 400);
        assert_eq!(totals.total_output_tokens(), 100);
        assert_eq!(totals.total_cache_hit_tokens(), 200);
        assert_eq!(totals.cache_hit_rate(), 0.5);
        assert_eq!(totals.last_input_tokens(), 300);
        assert_eq!(totals.context_window_tokens(), 1_000);
    }

    #[test]
    fn assistant_message_updates_token_totals_and_history_event() {
        let (mut app, agent) = test_app();
        assert!(app.world_mut().insert_component(
            agent,
            AgentContext {
                system_prompt: String::new(),
                messages: Vec::new(),
                tool_context: Vec::new(),
                ordered_messages: Vec::new(),
            },
        ));
        app.world_mut()
            .attach_agent_mcl(
                agent,
                AttachAgentMclRequest {
                    base: base_mcl(),
                    system_prompt: "system".into(),
                    context_window_tokens: 1_000_000,
                    restored_messages: Vec::new(),
                    default_visibility: BTreeSet::new(),
                },
            )
            .unwrap();
        assert!(app.world_mut().insert_component(
            agent,
            AgentTokenUsage::from_totals(&TokenUsage::default(), 0, 1_000_000),
        ));
        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .begin_turn("turn-1".into())
            .is_ok());

        let usage = TokenUsage {
            input_tokens: 80,
            output_tokens: 20,
            cache_hit_tokens: 40,
        };
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Assistant {
                reasoning: None,
                content: Some("done".into()),
                tool_calls: Vec::new(),
            },
            usage: Some(usage.clone()),
        });
        app.tick();
        app.world().send_event(MclHistoryAppendRequested {
            id: "turn-1".into(),
            agent,
            message: MclMessage::new(
                Message::Assistant {
                    reasoning: None,
                    content: Some("done".into()),
                    tool_calls: Vec::new(),
                },
                Some(usage.clone()),
            ),
        });
        app.tick();
        app.tick();

        let totals = app.world().get_component::<AgentTokenUsage>(agent).unwrap();
        assert_eq!(totals.total_input_tokens(), 80);
        assert_eq!(totals.total_output_tokens(), 20);
        assert_eq!(totals.total_cache_hit_tokens(), 40);
        assert_eq!(totals.cache_hit_rate(), 0.5);
        assert_eq!(totals.last_input_tokens(), 80);
        assert_eq!(totals.context_window_tokens(), 1_000_000);
        assert!(app
            .world()
            .event_reader::<AgentHistoryMessageWriteRequested>()
            .into_iter()
            .any(|event| event.id == "turn-1" && event.usage == Some(usage.clone())));
    }

    #[test]
    fn context_compaction_summarizes_the_head_and_keeps_the_requested_tail() {
        let (mut app, agent) = test_app();
        assert!(app.world_mut().insert_component(
            agent,
            AgentIdentity {
                id: ResourceId::parse("agent:test/agent0:latest").unwrap(),
            },
        ));
        let original = vec![
            Message::User {
                content: "old request".into(),
            },
            Message::Assistant {
                reasoning: None,
                content: Some("old answer".into()),
                tool_calls: Vec::new(),
            },
            Message::User {
                content: "recent request".into(),
            },
        ];
        assert!(app.world_mut().insert_component(
            agent,
            AgentContext {
                system_prompt: "system".into(),
                messages: original.clone(),
                tool_context: Vec::new(),
                ordered_messages: original.clone(),
            },
        ));

        app.world().send_event(AgentContextCompactRequest {
            id: "compact-1".into(),
            agent,
            retain_messages: 1,
        });
        app.tick();
        app.tick();

        let request = app
            .world()
            .event_reader::<ContextCompactionInferenceRequest>()
            .into_iter()
            .find(|event| event.id == "compact-1")
            .unwrap();
        assert_eq!(request.messages.len(), 4);
        assert_eq!(
            request.messages[0],
            Message::System {
                content: "system".into()
            }
        );
        assert_eq!(&request.messages[1..3], &original[..2]);
        assert!(matches!(
            &request.messages[3],
            Message::User { content }
                if content == CONTEXT_COMPACTION_PROMPT
        ));
        assert_eq!(
            app.world()
                .get_component::<AgentContext>(agent)
                .unwrap()
                .messages(),
            original
        );
        assert_eq!(app.world().agent_is_working(agent), Some(true));

        app.world().send_event(ContextCompactionInferenceResponse {
            id: "compact-1".into(),
            agent,
            result: Ok("condensed context".into()),
        });
        app.tick();
        app.tick();

        let rewritten = app
            .world()
            .get_component::<AgentContext>(agent)
            .unwrap()
            .messages();
        assert_eq!(rewritten.len(), 2);
        assert!(matches!(
            &rewritten[0],
            Message::User { content }
                if content.contains("<compacted-summary>\ncondensed context\n</compacted-summary>")
        ));
        assert_eq!(rewritten[1], original[2]);
        assert_eq!(app.world().agent_is_working(agent), Some(false));
        let update = app
            .world()
            .event_reader::<AgentContextMessagesUpdated>()
            .into_iter()
            .last()
            .unwrap();
        assert_eq!(update.messages, rewritten);
        assert!(app
            .world()
            .event_reader::<AgentHistoryMessageWriteRequested>()
            .is_empty());
    }

    #[test]
    fn context_compaction_does_not_overwrite_a_changed_context() {
        let (mut app, agent) = test_app();
        assert!(app.world_mut().insert_component(
            agent,
            AgentIdentity {
                id: ResourceId::parse("agent:test/agent0:latest").unwrap(),
            },
        ));
        assert!(app.world_mut().insert_component(
            agent,
            AgentContext {
                system_prompt: "system".into(),
                messages: vec![
                    Message::User {
                        content: "old".into(),
                    },
                    Message::Assistant {
                        reasoning: None,
                        content: Some("recent".into()),
                        tool_calls: Vec::new(),
                    },
                ],
                tool_context: Vec::new(),
                ordered_messages: Vec::new(),
            },
        ));
        app.world().send_event(AgentContextCompactRequest {
            id: "compact-1".into(),
            agent,
            retain_messages: 1,
        });
        app.tick();
        app.tick();
        let events = app.world().event_sender();
        app.world_mut()
            .get_component_mut::<AgentContext>(agent)
            .unwrap()
            .append_message(
                agent,
                Message::User {
                    content: "concurrent".into(),
                },
                &events,
            );
        app.world().send_event(ContextCompactionInferenceResponse {
            id: "compact-1".into(),
            agent,
            result: Ok("stale summary".into()),
        });
        app.tick();
        app.tick();

        let messages = app
            .world()
            .get_component::<AgentContext>(agent)
            .unwrap()
            .messages();
        assert_eq!(messages.len(), 3);
        assert!(matches!(&messages[2], Message::User { content, .. } if content == "concurrent"));
        assert_eq!(app.world().agent_is_working(agent), Some(false));
        assert!(app
            .world()
            .event_reader::<AgentFailure>()
            .into_iter()
            .any(|failure| failure.id == "compact-1"
                && failure.message == "ContextChanged: Agent context changed during compaction"));
    }

    #[test]
    fn aborting_a_turn_clears_state_and_requests_execution_cancellation() {
        let (mut app, agent) = test_app();
        assert!(app.world_mut().insert_component(
            agent,
            AgentContext {
                system_prompt: "system".into(),
                messages: Vec::new(),
                tool_context: vec![Message::Tool {
                    resource_id: ResourceId::parse("shell:local/bash:latest").unwrap(),
                    tool_call_id: "call-1".into(),
                    content: "partial".into(),
                }],
                ordered_messages: vec![Message::Tool {
                    resource_id: ResourceId::parse("shell:local/bash:latest").unwrap(),
                    tool_call_id: "call-1".into(),
                    content: "partial".into(),
                }],
            },
        ));
        assert!(app
            .world_mut()
            .get_component_mut::<AgentStatus>(agent)
            .unwrap()
            .begin_turn("turn-1".into())
            .is_ok());
        app.world_mut()
            .get_resource_mut::<PendingInferenceToolSchemas>()
            .unwrap()
            .schemas
            .insert((agent, "turn-1".into()), Vec::new());
        app.world_mut()
            .get_resource_mut::<PendingContextCompactions>()
            .unwrap()
            .requests
            .insert(
                (agent, "turn-1".into()),
                PendingContextCompaction {
                    original_messages: Vec::new(),
                    retained_messages: Vec::new(),
                },
            );

        app.world().send_event(AbortAgentTurn {
            id: "abort-1".into(),
            agent,
        });
        app.tick();
        app.tick();

        assert_eq!(app.world().agent_is_working(agent), Some(false));
        assert!(app
            .world()
            .get_component::<AgentContext>(agent)
            .unwrap()
            .tool_context()
            .is_empty());
        assert!(app
            .world()
            .get_resource::<PendingInferenceToolSchemas>()
            .unwrap()
            .schemas
            .is_empty());
        assert!(app
            .world()
            .get_resource::<PendingContextCompactions>()
            .unwrap()
            .requests
            .is_empty());
        assert!(app
            .world()
            .event_reader::<CancelInferenceRequest>()
            .into_iter()
            .any(|event| event.id == "turn-1" && event.agent == agent));
        assert!(app
            .world()
            .event_reader::<CancelToolTurn>()
            .into_iter()
            .any(|event| event.turn_id == "turn-1" && event.agent == agent));
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
            usage: None,
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
    fn tool_history_redacts_only_skill_response_content() {
        let (mut app, agent) = test_app();
        let events = app.world().event_sender();
        let messages = [
            AgentMessage {
                id: "skill-turn".into(),
                agent,
                message: Message::Tool {
                    resource_id: ResourceId::parse("skill:local/review:latest").unwrap(),
                    tool_call_id: "skill-call".into(),
                    content: "private skill instructions".into(),
                },
                usage: None,
            },
            AgentMessage {
                id: "tool-turn".into(),
                agent,
                message: Message::Tool {
                    resource_id: ResourceId::parse("shell:local/bash:latest").unwrap(),
                    tool_call_id: "tool-call".into(),
                    content: "complete command output".into(),
                },
                usage: None,
            },
        ];

        for message in &messages {
            record_history_message(app.world_mut(), message, &events, Vec::new());
        }
        app.tick();

        let writes = app
            .world()
            .event_reader::<AgentHistoryMessageWriteRequested>()
            .into_iter()
            .collect::<Vec<_>>();
        assert!(writes.iter().any(|write| matches!(
            &write.message,
            Message::Tool { content, .. } if write.id == "skill-turn"
                && content == "skill:local/review:latest"
        )));
        assert!(writes.iter().any(|write| matches!(
            &write.message,
            Message::Tool { content, .. } if write.id == "tool-turn"
                && content == "complete command output"
        )));
    }

    #[test]
    fn hidden_assistant_tool_call_becomes_a_denied_tool_message() {
        let (mut app, agent) = test_app();
        let resource_id = ResourceId::parse("shell:local/bash:latest").unwrap();
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
            AgentContext {
                system_prompt: "system".into(),
                messages: Vec::new(),
                tool_context: Vec::new(),
                ordered_messages: Vec::new(),
            },
        ));
        app.world_mut()
            .attach_agent_mcl(
                agent,
                AttachAgentMclRequest {
                    base: base_mcl(),
                    system_prompt: "system".into(),
                    context_window_tokens: 1_000_000,
                    restored_messages: Vec::new(),
                    default_visibility: BTreeSet::new(),
                },
            )
            .unwrap();
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

        let call = ToolCall {
            id: "call-1".into(),
            tool_name: map.tool_name,
            arguments: "{}".into(),
        };
        app.world().send_event(AgentMessage {
            id: "turn-1".into(),
            agent,
            message: Message::Assistant {
                reasoning: None,
                content: None,
                tool_calls: vec![call.clone()],
            },
            usage: None,
        });
        app.tick();
        app.world().send_event(MclEffectsProduced {
            id: "turn-1".into(),
            agent,
            effects: vec![MclEffect::ExecuteTools { calls: vec![call] }],
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
    }

    #[test]
    fn removing_a_resource_while_registration_is_in_flight_prevents_late_injection() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ToolPlugin::default())
            .add_plugin(mcl_plugin::MclPlugin::open(std::env::temp_dir()).unwrap())
            .add_plugin(AgentPlugin::default());
        let workspace = app.world_mut().spawn();
        let resource_id = ResourceId::parse("skill:local/review:latest").unwrap();
        app.world().send_event(AgentCreateRequest {
            id: "agent-create".into(),
            agent_id: ResourceId::parse("agent:demo/reviewer:latest").unwrap(),
            workspace_id: workspace,
            base_mcl: base_mcl(),
            system_prompt: String::new(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            ordered_messages: Vec::new(),
            token_usage: TokenUsage::default(),
            last_input_tokens: 0,
            context_window_tokens: 1_000_000,
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
            .get_component::<AgentMcl>(agent)
            .unwrap()
            .capabilities()
            .is_visible(&resource_id));
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
    fn external_visibility_switch_rejects_non_default_resources() {
        let (mut app, agent) = test_app();
        app.world_mut()
            .attach_agent_mcl(
                agent,
                AttachAgentMclRequest {
                    base: base_mcl(),
                    system_prompt: String::new(),
                    context_window_tokens: 1_000_000,
                    restored_messages: Vec::new(),
                    default_visibility: BTreeSet::new(),
                },
            )
            .unwrap();
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
            .get_component::<AgentMcl>(agent)
            .unwrap()
            .capabilities()
            .visible_resources()
            .next()
            .is_none());
    }
}
