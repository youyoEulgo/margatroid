use std::fmt;
use std::path::PathBuf;

use agent_plugin::{AgentIdentity, AgentTokenUsage, WorldAgentExt};
use core_plugin::{Entity, World};
use log_plugin::{TracingField, TracingRecord};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, AgentVisibilityRouteAction, Message, ResourceId,
    RouteAgentMessage, RouteAgentTurnAbort, RouteAgentVisibility, RouteAgentWorkflowAttach,
    RouteAgentWorkflowDetach, StartWorkspace, ToolCall, WorkspaceAgentDefinition,
    WorkspaceDefinition, WorkspaceReference,
};
use mcl_plugin::AgentMcl;
use memory_plugin::{AgentMemory, HistoryMessage};
use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};
use workspace_plugin::{
    StopWorkspaceByReference, WorkspaceAgentState, WorkspaceAgents, WorkspaceConfiguration,
    WorldWorkspaceExt,
};

const MAX_ERROR_MESSAGE_BYTES: usize = 512;

pub trait FromDomain<Domain, Context = ()>: Sized {
    fn from_domain(domain: Domain, context: Context) -> Result<Self, ProtocolError>;
}

pub trait IntoDomain<Domain, Context = ()>: Sized {
    fn into_domain(self, context: Context) -> Result<Domain, ProtocolError>;
}

pub trait IntoDto<Dto, Context = ()>: Sized {
    fn into_dto(self, context: Context) -> Result<Dto, ProtocolError>;
}

impl<Domain, Dto, Context> IntoDto<Dto, Context> for Domain
where
    Dto: FromDomain<Domain, Context>,
{
    fn into_dto(self, context: Context) -> Result<Dto, ProtocolError> {
        Dto::from_domain(self, context)
    }
}

pub trait FromDto<Dto, Context = ()>: Sized {
    fn from_dto(dto: Dto, context: Context) -> Result<Self, ProtocolError>;
}

impl<Domain, Dto, Context> FromDto<Dto, Context> for Domain
where
    Dto: IntoDomain<Domain, Context>,
{
    fn from_dto(dto: Dto, context: Context) -> Result<Self, ProtocolError> {
        dto.into_domain(context)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "connection.register")]
    ConnectionRegister {
        id: String,
        message: RegisterConnectionDto,
    },
    #[serde(rename = "workspace.start")]
    WorkspaceStart {
        id: String,
        message: StartWorkspaceDto,
    },
    #[serde(rename = "workspace.stop")]
    WorkspaceStop {
        id: String,
        message: StopWorkspaceDto,
    },
    #[serde(rename = "agent.message")]
    AgentMessage {
        id: String,
        message: RouteAgentMessageDto,
    },
    #[serde(rename = "agent.turn.abort")]
    AgentTurnAbort {
        id: String,
        message: RouteAgentTargetDto,
    },
    #[serde(rename = "agent.visibility.inject")]
    AgentVisibilityInject {
        id: String,
        message: RouteAgentVisibilityDto,
    },
    #[serde(rename = "agent.visibility.remove")]
    AgentVisibilityRemove {
        id: String,
        message: RouteAgentVisibilityDto,
    },
    #[serde(rename = "agent.workflow.attach")]
    AgentWorkflowAttach {
        id: String,
        message: RouteAgentWorkflowAttachDto,
    },
    #[serde(rename = "agent.workflow.detach")]
    AgentWorkflowDetach {
        id: String,
        message: RouteAgentWorkflowDetachDto,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "log")]
    Log { record: LogRecordDto },
    #[serde(rename = "state.sync")]
    StateSync { state: BackendStateDto },
    #[serde(rename = "workspace.started")]
    WorkspaceStarted {
        id: String,
        workspace: WorkspaceInfoDto,
    },
    #[serde(rename = "workspace.start_failed")]
    WorkspaceStartFailed { id: String, error: String },
    #[serde(rename = "workspace.stopped")]
    WorkspaceStopped {
        id: String,
        workspace: WorkspaceReferenceDto,
    },
    #[serde(rename = "workspace.stop_failed")]
    WorkspaceStopFailed { id: String, error: String },
    #[serde(rename = "agent.message")]
    AgentMessage { message: AgentMessageDto },
    #[serde(rename = "agent.message.delta")]
    AgentMessageDelta {
        id: String,
        agent: ResourceIdDto,
        content: String,
    },
    #[serde(rename = "agent.message.reasoning_delta")]
    AgentMessageReasoningDelta {
        id: String,
        agent: ResourceIdDto,
        content: String,
    },
    #[serde(rename = "agent.failure")]
    AgentFailure { failure: AgentFailureDto },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecordDto {
    pub timestamp_millis: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: Vec<LogFieldDto>,
    #[serde(default)]
    pub spans: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFieldDto {
    pub name: String,
    pub value: String,
}

impl FromDomain<TracingField> for LogFieldDto {
    fn from_domain(field: TracingField, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: field.name,
            value: field.value,
        })
    }
}

impl FromDomain<TracingRecord> for LogRecordDto {
    fn from_domain(record: TracingRecord, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            timestamp_millis: record.timestamp_millis,
            level: record.level,
            target: record.target,
            message: record.message,
            fields: record
                .fields
                .into_iter()
                .map(|field| field.into_dto(()))
                .collect::<Result<Vec<_>, _>>()?,
            spans: record.spans,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserMessageDto {
    pub content: String,
}

impl IntoDomain<Message> for UserMessageDto {
    fn into_domain(self, (): ()) -> Result<Message, ProtocolError> {
        Ok(Message::User {
            content: self.content,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallDto {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
}

impl FromDomain<&ToolCall> for ToolCallDto {
    fn from_domain(call: &ToolCall, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: call.id.clone(),
            tool_name: call.tool_name.clone(),
            arguments: call.arguments.clone(),
        })
    }
}

impl IntoDomain<ToolCall> for ToolCallDto {
    fn into_domain(self, (): ()) -> Result<ToolCall, ProtocolError> {
        Ok(ToolCall {
            id: self.id,
            tool_name: self.tool_name,
            arguments: self.arguments,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDto {
    User {
        content: String,
    },
    Assistant {
        #[serde(default)]
        reasoning: Option<String>,
        content: Option<String>,
        tool_calls: Vec<ToolCallDto>,
    },
    Tool {
        resource_id: ResourceIdDto,
        tool_call_id: String,
        content: String,
    },
}

impl FromDomain<&Message> for MessageDto {
    fn from_domain(message: &Message, (): ()) -> Result<Self, ProtocolError> {
        match message {
            Message::User { content } => Ok(Self::User {
                content: content.clone(),
            }),
            Message::Assistant {
                reasoning,
                content,
                tool_calls,
            } => Ok(Self::Assistant {
                reasoning: reasoning.clone(),
                content: content.clone(),
                tool_calls: tool_calls
                    .iter()
                    .map(|call| call.into_dto(()))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Message::Tool {
                resource_id,
                tool_call_id,
                content,
            } => Ok(Self::Tool {
                resource_id: ResourceIdDto::from_domain(resource_id, ())?,
                tool_call_id: tool_call_id.clone(),
                content: content.clone(),
            }),
            Message::System { .. } => Err(ProtocolError::new(
                ProtocolErrorKind::UnsupportedMessage,
                "system messages are not externally visible",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterConnectionDto {
    pub client_type: String,
}

impl IntoDomain<RegisterConnection, (String, WebSocketConnectionId)> for RegisterConnectionDto {
    fn into_domain(
        self,
        (id, connection_id): (String, WebSocketConnectionId),
    ) -> Result<RegisterConnection, ProtocolError> {
        Ok(RegisterConnection {
            id,
            connection_id,
            client_type: self.client_type,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWorkspaceDto {
    pub definition: WorkspaceDefinitionDto,
}

impl IntoDomain<StartWorkspace, String> for StartWorkspaceDto {
    fn into_domain(self, id: String) -> Result<StartWorkspace, ProtocolError> {
        Ok(StartWorkspace {
            id,
            definition: self.definition.into_domain(())?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopWorkspaceDto {
    pub workspace: WorkspaceReferenceDto,
}

impl IntoDomain<StopWorkspaceByReference, String> for StopWorkspaceDto {
    fn into_domain(self, id: String) -> Result<StopWorkspaceByReference, ProtocolError> {
        Ok(StopWorkspaceByReference {
            id,
            workspace: self.workspace.into_domain(())?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentMessageDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub message: UserMessageDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentTargetDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentVisibilityDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub resource_id: ResourceIdDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentWorkflowAttachDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub resource_id: ResourceIdDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentWorkflowDetachDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub instance_id: String,
}

impl IntoDomain<RouteAgentVisibility, (String, AgentVisibilityRouteAction)>
    for RouteAgentVisibilityDto
{
    fn into_domain(
        self,
        (id, action): (String, AgentVisibilityRouteAction),
    ) -> Result<RouteAgentVisibility, ProtocolError> {
        Ok(RouteAgentVisibility {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
            resource_id: self.resource_id.into_domain(())?,
            action,
        })
    }
}

impl IntoDomain<RouteAgentWorkflowAttach, String> for RouteAgentWorkflowAttachDto {
    fn into_domain(self, id: String) -> Result<RouteAgentWorkflowAttach, ProtocolError> {
        Ok(RouteAgentWorkflowAttach {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
            resource_id: self.resource_id.into_domain(())?,
        })
    }
}

impl IntoDomain<RouteAgentWorkflowDetach, String> for RouteAgentWorkflowDetachDto {
    fn into_domain(self, id: String) -> Result<RouteAgentWorkflowDetach, ProtocolError> {
        if self.instance_id.is_empty() || self.instance_id.chars().any(char::is_control) {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "Workflow instance ID is invalid",
            ));
        }
        Ok(RouteAgentWorkflowDetach {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
            instance_id: self.instance_id,
        })
    }
}

impl IntoDomain<RouteAgentMessage, String> for RouteAgentMessageDto {
    fn into_domain(self, id: String) -> Result<RouteAgentMessage, ProtocolError> {
        Ok(RouteAgentMessage {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
            message: Message::User {
                content: self.message.content,
            },
        })
    }
}

impl IntoDomain<RouteAgentTurnAbort, String> for RouteAgentTargetDto {
    fn into_domain(self, id: String) -> Result<RouteAgentTurnAbort, ProtocolError> {
        Ok(RouteAgentTurnAbort {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
        })
    }
}

impl ClientMessage {
    pub fn register_connection(id: impl Into<String>, client_type: impl Into<String>) -> Self {
        Self::ConnectionRegister {
            id: id.into(),
            message: RegisterConnectionDto {
                client_type: client_type.into(),
            },
        }
    }

    pub fn start_workspace(id: impl Into<String>, definition: &WorkspaceDefinition) -> Self {
        Self::WorkspaceStart {
            id: id.into(),
            message: StartWorkspaceDto {
                definition: definition
                    .into_dto(())
                    .expect("WorkspaceDefinition conversion cannot fail"),
            },
        }
    }

    pub fn stop_workspace(id: impl Into<String>, workspace: &WorkspaceReferenceDto) -> Self {
        Self::WorkspaceStop {
            id: id.into(),
            message: StopWorkspaceDto {
                workspace: workspace.clone(),
            },
        }
    }

    pub fn agent_message(
        id: impl Into<String>,
        workspace: &WorkspaceReferenceDto,
        agent: Option<ResourceIdDto>,
        content: impl Into<String>,
    ) -> Self {
        Self::AgentMessage {
            id: id.into(),
            message: RouteAgentMessageDto {
                workspace: workspace.clone(),
                agent,
                message: UserMessageDto {
                    content: content.into(),
                },
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceReferenceDto {
    pub id: ResourceIdDto,
    pub name: String,
    pub project_root: String,
}

impl WorkspaceReferenceDto {
    pub fn new(name: impl Into<String>, project_root: impl Into<String>) -> Self {
        let name = name.into();
        let id = ResourceId::new("workspace", "local", &name, None::<String>)
            .expect("workspace reference name must be valid");
        Self::with_id(ResourceIdDto(id.to_string()), name, project_root)
    }

    pub fn with_id(
        id: ResourceIdDto,
        name: impl Into<String>,
        project_root: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            project_root: project_root.into(),
        }
    }
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceReferenceDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self::with_id(
            (&definition.id).into_dto(())?,
            definition.name.clone(),
            definition.project_root.to_string_lossy().into_owned(),
        ))
    }
}

impl FromDomain<WorkspaceReference> for WorkspaceReferenceDto {
    fn from_domain(reference: WorkspaceReference, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self::with_id(
            (&reference.id).into_dto(())?,
            reference.name,
            reference.project_root.to_string_lossy().into_owned(),
        ))
    }
}

impl IntoDomain<WorkspaceReference> for WorkspaceReferenceDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceReference, ProtocolError> {
        Ok(WorkspaceReference {
            id: self.id.into_domain(())?,
            name: self.name,
            project_root: PathBuf::from(self.project_root),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    pub id: ResourceIdDto,
    pub name: String,
    pub project_root: String,
    pub manager: ResourceIdDto,
    pub agents: Vec<ResourceIdDto>,
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceInfoDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: agent_resource_id(&definition.name, &definition.manager)?.into_dto(())?,
            agents: definition
                .agents
                .iter()
                .map(|agent| agent_resource_id(&definition.name, &agent.name)?.into_dto(()))
                .collect::<Result<_, ProtocolError>>()?,
        })
    }
}

impl WorkspaceInfoDto {
    pub fn reference(&self) -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::with_id(
            self.id.clone(),
            self.name.clone(),
            self.project_root.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackendStateDto {
    pub workspaces: Vec<WorkspaceInfoDto>,
    #[serde(default)]
    pub agents: Vec<AgentStateDto>,
    #[serde(default)]
    pub histories: Vec<AgentHistoryDto>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStateDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub status: WorkspaceAgentStatusDto,
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub default_resources: Vec<ResourceIdDto>,
    pub visible_resources: Vec<ResourceIdDto>,
    #[serde(default)]
    pub mcl: Option<AgentMclStateDto>,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cache_hit_tokens: u64,
    #[serde(default)]
    pub cache_hit_rate: f64,
    #[serde(default)]
    pub last_input_tokens: u64,
    #[serde(default)]
    pub context_window_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMclStateDto {
    pub base: ResourceIdDto,
    pub base_program_hash: String,
    pub plan_hash: String,
    pub plan_generation: u64,
    pub workflows: Vec<WorkflowMclStateDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowMclStateDto {
    pub instance_id: String,
    pub resource_id: ResourceIdDto,
    pub program_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAgentStatusDto {
    Creating,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHistoryDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub messages: Vec<HistoryMessageDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessageDto {
    pub sequence: i64,
    pub turn_id: String,
    pub message: MessageDto,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageDto {
    pub id: String,
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub message: MessageDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFailureDto {
    pub id: String,
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub kind: String,
    pub message: String,
}

impl FromDomain<HistoryMessage> for HistoryMessageDto {
    fn from_domain(message: HistoryMessage, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            sequence: message.sequence,
            turn_id: message.turn_id,
            message: (&message.message).into_dto(())?,
            created_at_ms: message.created_at_ms,
        })
    }
}

impl FromDomain<&AgentMessage, &World> for AgentMessageDto {
    fn from_domain(message: &AgentMessage, world: &World) -> Result<Self, ProtocolError> {
        let (workspace, agent) = agent_route(world, message.agent)?;
        Ok(Self {
            id: message.id.clone(),
            workspace,
            agent,
            message: (&message.message).into_dto(())?,
        })
    }
}

impl FromDomain<&AgentFailure, &World> for AgentFailureDto {
    fn from_domain(failure: &AgentFailure, world: &World) -> Result<Self, ProtocolError> {
        let (workspace, agent) = agent_route(world, failure.agent)?;
        let kind = match failure.kind {
            AgentFailureKind::Agent => "agent",
            AgentFailureKind::Inference => "inference",
        };
        Ok(Self {
            id: failure.id.clone(),
            workspace,
            agent,
            kind: kind.into(),
            message: failure.message.clone(),
        })
    }
}

impl FromDomain<Entity, &World> for WorkspaceInfoDto {
    fn from_domain(workspace: Entity, world: &World) -> Result<Self, ProtocolError> {
        let configuration = world
            .get_component::<WorkspaceConfiguration>(workspace)
            .ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorKind::WorkspaceNotFound,
                    "workspace configuration is missing",
                )
            })?;
        configuration.definition().into_dto(())
    }
}

impl FromDomain<(Entity, &str, &WorkspaceInfoDto), &World> for AgentStateDto {
    fn from_domain(
        (agent, _name, workspace): (Entity, &str, &WorkspaceInfoDto),
        world: &World,
    ) -> Result<Self, ProtocolError> {
        let mcl = world.get_component::<AgentMcl>(agent).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::AgentNotFound,
                "Agent MCL state is missing",
            )
        })?;
        let default_resources = mcl
            .capabilities()
            .default_resources()
            .iter()
            .map(|resource| resource.into_dto(()))
            .collect::<Result<Vec<_>, _>>()?;
        let visible_resources = mcl
            .capabilities()
            .visible_resources()
            .map(|resource| resource.into_dto(()))
            .collect::<Result<Vec<_>, _>>()?;
        let mcl = AgentMclStateDto {
            base: mcl.base().resource_id().into_dto(())?,
            base_program_hash: mcl.base().plan_hash().to_string(),
            plan_hash: mcl.plan_hash().to_string(),
            plan_generation: mcl.plan_generation(),
            workflows: mcl
                .workflows()
                .map(|(instance_id, workflow)| {
                    Ok(WorkflowMclStateDto {
                        instance_id: instance_id.to_string(),
                        resource_id: workflow.resource_id().into_dto(())?,
                        program_hash: workflow.program().plan_hash().to_string(),
                    })
                })
                .collect::<Result<_, ProtocolError>>()?,
        };
        let identity = world.get_component::<AgentIdentity>(agent).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::AgentNotFound,
                "Agent identity is missing",
            )
        })?;
        let working = world.agent_is_working(agent).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::AgentNotFound,
                "Agent work status is missing",
            )
        })?;
        let token_usage = world
            .get_component::<AgentTokenUsage>(agent)
            .ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorKind::AgentNotFound,
                    "Agent token usage is missing",
                )
            })?;
        Ok(Self {
            workspace: workspace.reference(),
            agent: identity.id().into_dto(())?,
            status: WorkspaceAgentStatusDto::Ready,
            working,
            error: None,
            default_resources,
            visible_resources,
            mcl: Some(mcl),
            total_input_tokens: token_usage.total_input_tokens(),
            total_output_tokens: token_usage.total_output_tokens(),
            total_cache_hit_tokens: token_usage.total_cache_hit_tokens(),
            cache_hit_rate: token_usage.cache_hit_rate(),
            last_input_tokens: token_usage.last_input_tokens(),
            context_window_tokens: token_usage.context_window_tokens(),
        })
    }
}

impl FromDomain<(), &World> for BackendStateDto {
    fn from_domain((): (), world: &World) -> Result<Self, ProtocolError> {
        let mut workspaces = world
            .workspaces()
            .into_iter()
            .map(|workspace| Ok((workspace, workspace.into_dto(world)?)))
            .collect::<Result<Vec<(Entity, WorkspaceInfoDto)>, ProtocolError>>()?;
        workspaces.sort_by(|left, right| {
            left.1
                .project_root
                .cmp(&right.1.project_root)
                .then_with(|| left.1.name.cmp(&right.1.name))
        });

        let mut workspace_infos = Vec::with_capacity(workspaces.len());
        let mut agent_states = Vec::new();
        let mut histories = Vec::new();
        for (workspace, info) in workspaces {
            let agents = world
                .get_component::<WorkspaceAgents>(workspace)
                .ok_or_else(|| {
                    ProtocolError::new(
                        ProtocolErrorKind::WorkspaceNotFound,
                        "workspace Agent index is missing",
                    )
                })?;
            let configuration = world
                .get_component::<WorkspaceConfiguration>(workspace)
                .ok_or_else(|| {
                    ProtocolError::new(
                        ProtocolErrorKind::WorkspaceNotFound,
                        "workspace configuration is missing",
                    )
                })?;
            for (name, state) in agents.states() {
                let definition = configuration
                    .definition()
                    .agents
                    .iter()
                    .find(|agent| agent.name == name)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ProtocolErrorKind::AgentNotFound,
                            "workspace Agent state is not present in its definition",
                        )
                    })?;
                match state {
                    WorkspaceAgentState::Ready { agent } => {
                        agent_states.push(AgentStateDto::from_domain((*agent, name, &info), world)?)
                    }
                    WorkspaceAgentState::Creating => agent_states.push(AgentStateDto {
                        workspace: info.reference(),
                        agent: (&definition.id).into_dto(())?,
                        status: WorkspaceAgentStatusDto::Creating,
                        working: false,
                        error: None,
                        default_resources: Vec::new(),
                        visible_resources: Vec::new(),
                        mcl: None,
                        total_input_tokens: 0,
                        total_output_tokens: 0,
                        total_cache_hit_tokens: 0,
                        cache_hit_rate: 0.0,
                        last_input_tokens: 0,
                        context_window_tokens: 0,
                    }),
                    WorkspaceAgentState::Failed { error } => agent_states.push(AgentStateDto {
                        workspace: info.reference(),
                        agent: (&definition.id).into_dto(())?,
                        status: WorkspaceAgentStatusDto::Failed,
                        working: false,
                        error: Some(error.to_string()),
                        default_resources: Vec::new(),
                        visible_resources: Vec::new(),
                        mcl: None,
                        total_input_tokens: 0,
                        total_output_tokens: 0,
                        total_cache_hit_tokens: 0,
                        cache_hit_rate: 0.0,
                        last_input_tokens: 0,
                        context_window_tokens: 0,
                    }),
                }
            }
            for (_name, agent) in agents.iter() {
                let memory = world.get_component::<AgentMemory>(agent).ok_or_else(|| {
                    ProtocolError::new(ProtocolErrorKind::MemoryNotFound, "Agent memory is missing")
                })?;
                let messages = memory
                    .history_messages()
                    .map_err(|error| {
                        ProtocolError::new(
                            ProtocolErrorKind::MemoryReadFailed,
                            format!("Agent history could not be read: {error}"),
                        )
                    })?
                    .into_iter()
                    .map(|message| message.into_dto(()))
                    .collect::<Result<Vec<_>, _>>()?;
                histories.push(AgentHistoryDto {
                    workspace: info.reference(),
                    agent: world
                        .get_component::<AgentIdentity>(agent)
                        .ok_or_else(|| {
                            ProtocolError::new(
                                ProtocolErrorKind::AgentNotFound,
                                "Agent identity is missing",
                            )
                        })?
                        .id()
                        .into_dto(())?,
                    messages,
                });
            }
            workspace_infos.push(info);
        }
        Ok(Self {
            workspaces: workspace_infos,
            agents: agent_states,
            histories,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDefinitionDto {
    pub id: ResourceIdDto,
    pub name: String,
    pub project_root: String,
    pub manager: String,
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(|agent| agent.into_dto(()))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl IntoDomain<WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceDefinition, ProtocolError> {
        let agents = self
            .agents
            .into_iter()
            .map(|agent| agent.into_domain(()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceDefinition {
            id: self.id.into_domain(())?,
            name: self.name,
            project_root: PathBuf::from(self.project_root),
            manager: self.manager,
            agents,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub id: ResourceIdDto,
    pub image: ResourceIdDto,
    pub resources: Vec<ResourceIdDto>,
    pub disable_resources: Vec<ResourceIdDto>,
    pub memory_path: Option<String>,
}

impl FromDomain<&WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn from_domain(definition: &WorkspaceAgentDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: definition.name.clone(),
            id: (&definition.id).into_dto(())?,
            image: (&definition.image).into_dto(())?,
            resources: definition
                .resources
                .iter()
                .map(|resource| resource.into_dto(()))
                .collect::<Result<Vec<_>, _>>()?,
            disable_resources: definition
                .disable_resources
                .iter()
                .map(|resource| resource.into_dto(()))
                .collect::<Result<Vec<_>, _>>()?,
            memory_path: definition
                .memory_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        })
    }
}

impl IntoDomain<WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceAgentDefinition, ProtocolError> {
        let image = self.image.into_domain(())?;
        if image.resource_type() != "image" {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidImageReference,
                "AgentImage resource must use type image",
            ));
        }
        let resources = self
            .resources
            .into_iter()
            .map(|resource| resource.into_domain(()))
            .collect::<Result<Vec<_>, _>>()?;
        let disable_resources = self
            .disable_resources
            .into_iter()
            .map(|resource| resource.into_domain(()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceAgentDefinition {
            name: self.name,
            id: self.id.into_domain(())?,
            image,
            resources,
            disable_resources,
            memory_path: self.memory_path.map(PathBuf::from),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceIdDto(pub String);

impl fmt::Display for ResourceIdDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromDomain<&ResourceId> for ResourceIdDto {
    fn from_domain(resource: &ResourceId, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(resource.to_string()))
    }
}

impl FromDomain<ResourceId> for ResourceIdDto {
    fn from_domain(resource: ResourceId, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(resource.to_string()))
    }
}

impl IntoDomain<ResourceId> for ResourceIdDto {
    fn into_domain(self, (): ()) -> Result<ResourceId, ProtocolError> {
        ResourceId::parse(self.0).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidResourceReference,
                format!("invalid resource ID: {error}"),
            )
        })
    }
}

fn agent_route(
    world: &World,
    agent: Entity,
) -> Result<(WorkspaceReferenceDto, ResourceIdDto), ProtocolError> {
    let identity = world.get_component::<AgentIdentity>(agent).ok_or_else(|| {
        ProtocolError::new(
            ProtocolErrorKind::AgentNotFound,
            "Agent identity is missing",
        )
    })?;
    let workspace = world.workspace_of(agent).ok_or_else(|| {
        ProtocolError::new(
            ProtocolErrorKind::WorkspaceNotFound,
            "Agent workspace is missing",
        )
    })?;
    let configuration = world
        .get_component::<WorkspaceConfiguration>(workspace)
        .ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::WorkspaceNotFound,
                "workspace configuration is missing",
            )
        })?;
    let workspace = configuration.definition().into_dto(())?;
    Ok((workspace, identity.id().into_dto(())?))
}

fn agent_resource_id(workspace: &str, agent: &str) -> Result<ResourceId, ProtocolError> {
    ResourceId::new("agent", workspace, agent, None::<String>).map_err(|error| {
        ProtocolError::new(
            ProtocolErrorKind::InvalidResourceReference,
            format!("invalid Agent resource ID: {error}"),
        )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolErrorKind {
    AgentNotFound,
    InvalidRequest,
    InvalidImageReference,
    InvalidResourceReference,
    MemoryNotFound,
    MemoryReadFailed,
    UnsupportedMessage,
    WorkspaceNotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    kind: ProtocolErrorKind,
    message: String,
}

impl ProtocolError {
    fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_MESSAGE_BYTES {
            let suffix = "...";
            let mut boundary = MAX_ERROR_MESSAGE_BYTES - suffix.len();
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push_str(suffix);
        }
        Self { kind, message }
    }

    pub fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use agent_plugin::{AgentCreateRequest, AgentCreateResult, AgentPlugin, WorldAgentExt};
    use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
    use core_plugin::App;
    use mcl_plugin::{compile_mcl, MclCompileRequest, MclPlugin, MclSource};
    use serde_json::json;
    use tool_plugin::{register_agent_tool, ToolPlugin, ToolTemplate};

    use super::*;

    fn base_mcl() -> std::sync::Arc<mcl_plugin::MclProgram> {
        compile_mcl(MclCompileRequest {
            root: MclSource::new(
                ResourceId::parse("mcl:local/test:latest").unwrap(),
                r#"base context test {
block conversation: context persistent;
view messages: messages { select entry from conversation; }
view tools: tools { select resource from capabilities.dynamic; }
request inference { system = agent.system; messages = messages; tools = tools; }
on agent.created { restore capabilities.dynamic from capabilities.default; }
}"#,
                PathBuf::from("/test/main.mcl"),
            ),
            dependencies: BTreeMap::new(),
        })
        .unwrap()
    }

    fn definition() -> WorkspaceDefinition {
        WorkspaceDefinition {
            id: ResourceId::parse("workspace:local/demo").unwrap(),
            name: "demo".into(),
            project_root: Path::new("/tmp/demo").into(),
            manager: "coder".into(),
            agents: vec![WorkspaceAgentDefinition {
                name: "coder".into(),
                id: ResourceId::parse("agent:demo/coder:clone0").unwrap(),
                image: ResourceId::parse("image:local/coder:v1").unwrap(),
                resources: vec![ResourceId::parse("skill:local/context").unwrap()],
                disable_resources: Vec::new(),
                memory_path: None,
            }],
        }
    }

    fn workspace_reference() -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::with_id(
            ResourceIdDto("workspace:local/demo:latest".into()),
            "demo",
            "/tmp/demo",
        )
    }

    #[test]
    fn request_uses_stable_workspace_start_shape() {
        let value =
            serde_json::to_value(ClientMessage::start_workspace("request-1", &definition()))
                .unwrap();
        assert_eq!(value["type"], "workspace.start");
        assert_eq!(value["id"], "request-1");
        assert_eq!(
            value["message"]["definition"]["agents"][0]["id"],
            "agent:demo/coder:clone0"
        );
        assert_eq!(
            value["message"]["definition"]["agents"][0]["image"],
            "image:local/coder:v1"
        );
        assert_eq!(
            value["message"]["definition"]["agents"][0]["resources"][0],
            "skill:local/context:latest"
        );
    }

    #[test]
    fn connection_registration_uses_stable_shape() {
        let value = serde_json::to_value(ClientMessage::register_connection("register-1", "webui"))
            .unwrap();

        assert_eq!(value["type"], "connection.register");
        assert_eq!(value["id"], "register-1");
        assert_eq!(value["message"]["client_type"], "webui");
    }

    #[test]
    fn workspace_stop_uses_workspace_reference_and_request_id() {
        let workspace = workspace_reference();
        let value =
            serde_json::to_value(ClientMessage::stop_workspace("stop-1", &workspace)).unwrap();

        assert_eq!(value["type"], "workspace.stop");
        assert_eq!(value["id"], "stop-1");
        assert_eq!(value["message"]["workspace"]["name"], "demo");
        assert_eq!(value["message"]["workspace"]["project_root"], "/tmp/demo");
    }

    #[test]
    fn agent_visibility_inject_uses_stable_shape_and_domain_route() {
        let value = json!({
            "type": "agent.visibility.inject",
            "id": "visibility-1",
            "message": {
                "workspace": workspace_reference(),
                "agent": "agent:demo/coder:latest",
                "resource_id": "skill:local/review:latest"
            }
        });
        let request: ClientMessage = serde_json::from_value(value).unwrap();
        let ClientMessage::AgentVisibilityInject { id, message } = request else {
            panic!("expected agent visibility injection");
        };
        let route = message
            .into_domain((id, AgentVisibilityRouteAction::Inject))
            .unwrap();

        assert_eq!(route.id, "visibility-1");
        assert_eq!(route.workspace.name, "demo");
        assert_eq!(route.agent.unwrap().to_string(), "agent:demo/coder:latest");
        assert_eq!(route.resource_id.to_string(), "skill:local/review:latest");
        assert_eq!(route.action, AgentVisibilityRouteAction::Inject);
    }

    #[test]
    fn agent_turn_abort_uses_workspace_and_optional_agent_route() {
        let request: ClientMessage = serde_json::from_value(json!({
            "type": "agent.turn.abort",
            "id": "abort-1",
            "message": {
                "workspace": workspace_reference(),
                "agent": "agent:demo/coder:latest"
            }
        }))
        .unwrap();
        let ClientMessage::AgentTurnAbort { id, message } = request else {
            panic!("expected agent.turn.abort");
        };
        let route: RouteAgentTurnAbort = message.into_domain(id).unwrap();

        assert_eq!(route.id, "abort-1");
        assert_eq!(route.workspace.name, "demo");
        assert_eq!(route.agent.unwrap().to_string(), "agent:demo/coder:latest");
    }

    #[test]
    fn workspace_stop_result_uses_stable_server_shapes() {
        let workspace = workspace_reference();
        let stopped = serde_json::to_value(ServerMessage::WorkspaceStopped {
            id: "stop-1".into(),
            workspace: workspace.clone(),
        })
        .unwrap();
        assert_eq!(stopped["type"], "workspace.stopped");
        assert_eq!(stopped["id"], "stop-1");

        let failed = serde_json::to_value(ServerMessage::WorkspaceStopFailed {
            id: "stop-1".into(),
            error: "workspace is not started".into(),
        })
        .unwrap();
        assert_eq!(failed["type"], "workspace.stop_failed");
        assert_eq!(failed["error"], "workspace is not started");
    }

    #[test]
    fn dto_round_trips_to_domain_definition() {
        let original = definition();
        let dto: WorkspaceDefinitionDto = (&original).into_dto(()).unwrap();
        assert_eq!(WorkspaceDefinition::from_dto(dto, ()).unwrap(), original);
    }

    #[test]
    fn agent_message_uses_workspace_identity_and_optional_agent_id() {
        let workspace = workspace_reference();
        let value = serde_json::to_value(ClientMessage::agent_message(
            "message-1",
            &workspace,
            Some(ResourceIdDto("agent:demo/reviewer:latest".into())),
            "Review this change.",
        ))
        .unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["workspace"]["name"], "demo");
        assert_eq!(value["message"]["workspace"]["project_root"], "/tmp/demo");
        assert_eq!(value["message"]["agent"], "agent:demo/reviewer:latest");
        assert_eq!(
            value["message"]["message"]["content"],
            "Review this change."
        );
    }

    #[test]
    fn omitted_agent_is_encoded_for_manager_fallback() {
        let workspace = workspace_reference();
        let value = serde_json::to_value(ClientMessage::agent_message(
            "message-1",
            &workspace,
            None,
            "Hello.",
        ))
        .unwrap();

        assert!(value["message"]["agent"].is_null());
    }

    #[test]
    fn internal_messages_are_not_externally_visible() {
        let result: Result<MessageDto, _> = (&Message::System {
            content: "system".into(),
        })
            .into_dto(());
        assert_eq!(
            result.unwrap_err().kind(),
            ProtocolErrorKind::UnsupportedMessage
        );

        let tool = Message::Tool {
            resource_id: ResourceId::parse("skill:local/context:latest").unwrap(),
            tool_call_id: "call-1".into(),
            content: "result".into(),
        };
        assert!(matches!((&tool).into_dto(()), Ok(MessageDto::Tool { .. })));
    }

    #[test]
    fn workspace_started_exposes_manager_and_selectable_agents() {
        let event = ServerMessage::WorkspaceStarted {
            id: "request-1".into(),
            workspace: (&definition()).into_dto(()).unwrap(),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "workspace.started");
        assert_eq!(value["workspace"]["manager"], "agent:demo/coder:latest");
        assert_eq!(value["workspace"]["agents"][0], "agent:demo/coder:latest");
    }

    #[test]
    fn workspace_start_failure_uses_a_terminal_server_shape() {
        let event = ServerMessage::WorkspaceStartFailed {
            id: "request-1".into(),
            error: "ResourceSetupFailed: skill file was not found".into(),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "workspace.start_failed");
        assert_eq!(value["id"], "request-1");
        assert_eq!(
            value["error"],
            "ResourceSetupFailed: skill file was not found"
        );
    }

    #[test]
    fn state_sync_contains_the_complete_workspace_snapshot() {
        let event = ServerMessage::StateSync {
            state: BackendStateDto {
                workspaces: vec![(&definition()).into_dto(()).unwrap()],
                agents: vec![AgentStateDto {
                    workspace: workspace_reference(),
                    agent: ResourceIdDto("agent:demo/coder:latest".into()),
                    status: WorkspaceAgentStatusDto::Ready,
                    working: false,
                    error: None,
                    default_resources: vec![ResourceIdDto("skill:local/review:latest".into())],
                    visible_resources: vec![ResourceIdDto("skill:local/review:latest".into())],
                    mcl: None,
                    total_input_tokens: 1_000,
                    total_output_tokens: 200,
                    total_cache_hit_tokens: 750,
                    cache_hit_rate: 0.75,
                    last_input_tokens: 800,
                    context_window_tokens: 200_000,
                }],
                histories: vec![AgentHistoryDto {
                    workspace: workspace_reference(),
                    agent: ResourceIdDto("agent:demo/coder:latest".into()),
                    messages: vec![HistoryMessageDto {
                        sequence: 1,
                        turn_id: "turn-1".into(),
                        message: MessageDto::User {
                            content: "hello".into(),
                        },
                        created_at_ms: 42,
                    }],
                }],
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "state.sync");
        assert_eq!(value["state"]["workspaces"][0]["name"], "demo");
        assert_eq!(
            value["state"]["workspaces"][0]["manager"],
            "agent:demo/coder:latest"
        );
        assert_eq!(
            value["state"]["agents"][0]["agent"],
            "agent:demo/coder:latest"
        );
        assert_eq!(value["state"]["agents"][0]["working"], false);
        assert_eq!(value["state"]["agents"][0]["total_input_tokens"], 1_000);
        assert_eq!(value["state"]["agents"][0]["total_output_tokens"], 200);
        assert_eq!(value["state"]["agents"][0]["total_cache_hit_tokens"], 750);
        assert_eq!(value["state"]["agents"][0]["cache_hit_rate"], 0.75);
        assert_eq!(value["state"]["agents"][0]["last_input_tokens"], 800);
        assert_eq!(
            value["state"]["agents"][0]["context_window_tokens"],
            200_000
        );
        assert_eq!(
            value["state"]["agents"][0]["visible_resources"][0],
            "skill:local/review:latest"
        );
        assert_eq!(
            value["state"]["histories"][0]["agent"],
            "agent:demo/coder:latest"
        );
        assert_eq!(
            value["state"]["histories"][0]["messages"][0]["turn_id"],
            "turn-1"
        );
    }

    #[test]
    fn agent_state_projects_dynamic_visibility() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ToolPlugin::default())
            .add_plugin(MclPlugin::open(std::env::temp_dir()).unwrap())
            .add_plugin(AgentPlugin::default());
        let workspace = app.world_mut().spawn();
        let skill = ResourceId::parse("skill:local/review").unwrap();
        app.world().send_event(AgentCreateRequest {
            id: "create-1".into(),
            agent_id: ResourceId::parse("agent:demo/coder").unwrap(),
            workspace_id: workspace,
            base_mcl: base_mcl(),
            system_prompt: "system".into(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            ordered_messages: Vec::new(),
            token_usage: margatroid_types::TokenUsage {
                input_tokens: 400,
                output_tokens: 100,
                cache_hit_tokens: 250,
            },
            last_input_tokens: 400,
            context_window_tokens: 1_000_000,
            default_visibility: BTreeSet::from([skill.clone()]),
        });
        app.tick();
        app.tick();
        let agent = app
            .world()
            .event_reader::<AgentCreateResult>()
            .into_iter()
            .find(|event| event.id == "create-1")
            .unwrap()
            .result
            .as_ref()
            .copied()
            .unwrap();
        register_agent_tool(
            app.world_mut(),
            agent,
            ResourceId::parse("tool:builtin/skill-loader:latest").unwrap(),
            skill.clone(),
            ToolTemplate::new("placeholder", "Load a skill.", json!({"type":"object"})).unwrap(),
        )
        .unwrap();
        app.world()
            .inject_agent_visible_resource("inject-1", agent, skill);
        app.tick();
        app.tick();
        let workspace = (&definition()).into_dto(()).unwrap();

        let state = AgentStateDto::from_domain((agent, "coder", &workspace), app.world()).unwrap();

        assert_eq!(state.workspace.name, "demo");
        assert_eq!(state.agent, ResourceIdDto("agent:demo/coder:latest".into()));
        assert!(!state.working);
        assert_eq!(state.visible_resources.len(), 1);
        assert_eq!(state.default_resources.len(), 1);
        assert_eq!(state.total_input_tokens, 400);
        assert_eq!(state.total_output_tokens, 100);
        assert_eq!(state.total_cache_hit_tokens, 250);
        assert_eq!(state.cache_hit_rate, 0.625);
        assert_eq!(state.last_input_tokens, 400);
        assert_eq!(state.context_window_tokens, 1_000_000);
        assert_eq!(
            state.visible_resources[0],
            ResourceIdDto("skill:local/review:latest".into())
        );
    }

    #[test]
    fn agent_message_event_exposes_resolved_route_and_message() {
        let event = ServerMessage::AgentMessage {
            message: AgentMessageDto {
                id: "message-1".into(),
                workspace: workspace_reference(),
                agent: ResourceIdDto("agent:demo/coder:latest".into()),
                message: MessageDto::Assistant {
                    reasoning: Some("checking".into()),
                    content: Some("Done.".into()),
                    tool_calls: Vec::new(),
                },
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["agent"], "agent:demo/coder:latest");
        assert_eq!(
            value["message"]["message"]["Assistant"]["reasoning"],
            "checking"
        );
        assert_eq!(value["message"]["message"]["Assistant"]["content"], "Done.");
    }

    #[test]
    fn agent_message_delta_serializes_as_a_flat_stream_frame() {
        let event = ServerMessage::AgentMessageDelta {
            id: "turn-1".into(),
            agent: ResourceIdDto("agent:demo/coder:latest".into()),
            content: "hello".into(),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message.delta");
        assert_eq!(value["id"], "turn-1");
        assert_eq!(value["agent"], "agent:demo/coder:latest");
        assert_eq!(value["content"], "hello");
    }

    #[test]
    fn agent_reasoning_delta_serializes_as_a_flat_stream_frame() {
        let event = ServerMessage::AgentMessageReasoningDelta {
            id: "turn-1".into(),
            agent: ResourceIdDto("agent:demo/coder:latest".into()),
            content: "checking".into(),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message.reasoning_delta");
        assert_eq!(value["id"], "turn-1");
        assert_eq!(value["agent"], "agent:demo/coder:latest");
        assert_eq!(value["content"], "checking");
    }

    #[test]
    fn server_log_event_decodes_without_llm_fields() {
        let event: ServerMessage = serde_json::from_str(
            r#"{
                "type": "log",
                "record": {
                    "timestamp_millis": 1,
                    "level": "INFO",
                    "target": "workspace",
                    "message": "started"
                }
            }"#,
        )
        .unwrap();
        match event {
            ServerMessage::Log { record } => {
                assert_eq!(record.message, "started");
                assert!(record.fields.is_empty());
            }
            _ => panic!("expected a log event"),
        }
    }
}
