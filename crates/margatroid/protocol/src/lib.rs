use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use agent_plugin::{Agent, HistoryMessage};
use core_plugin::{Entity, World};
use log_plugin::{TracingField, TracingRecord};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentMessage, Message, ResourceId, ResourceRef,
    RouteAgentTurnAbort, RouteMclCommand, StartWorkspace, ToolCall, WorkspaceAgentDefinition,
    WorkspaceDefinition, WorkspaceReference,
};

use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};
use workspace_plugin::{
    StopWorkspaceByReference, Workspace, WorkspaceAgentState, WorldWorkspaceExt,
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
    #[serde(rename = "mcl.command")]
    MclCommand { id: String, message: MclCommandDto },
    #[serde(rename = "agent.turn.abort")]
    AgentTurnAbort {
        id: String,
        message: RouteAgentTargetDto,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "log")]
    Log { record: LogRecordDto },
    #[serde(rename = "state.sync")]
    StateSync { state: BackendStateDto },
    #[serde(rename = "connection.registered")]
    ConnectionRegistered { id: String, client: ClientInfoDto },
    #[serde(rename = "connection.register_failed")]
    ConnectionRegisterFailed { id: String, error: String },
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
    #[serde(rename = "mcl.command_result")]
    MclCommandResult {
        id: String,
        result: Result<serde_json::Value, String>,
    },
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

impl MessageDto {
    fn into_message(self, server_side_allowed: bool) -> Result<Message, ProtocolError> {
        match self {
            Self::User { content } => Ok(Message::User { content }),
            Self::Assistant {
                reasoning,
                content,
                tool_calls,
            } => Ok(Message::Assistant {
                reasoning,
                content,
                tool_calls: tool_calls
                    .into_iter()
                    .map(|call| call.into_domain(()))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Self::Tool {
                resource_id,
                tool_call_id,
                content,
                failed,
            } if server_side_allowed => Ok(Message::Tool {
                resource_id: resource_id.into_domain(())?,
                tool_call_id,
                content,
                failed,
            }),
            Self::Error { message } if server_side_allowed => Ok(Message::Error { message }),
            Self::Tool { .. } | Self::Error { .. } => Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "clients may only inject user or assistant messages",
            )),
            Self::Inject { messages } => Ok(Message::Inject {
                messages: messages
                    .into_iter()
                    .map(|inner| match inner {
                        Self::Inject { .. } => Err(ProtocolError::new(
                            ProtocolErrorKind::InvalidRequest,
                            "a context injection cannot nest another injection",
                        )),
                        inner => inner.into_message(true),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            }),
        }
    }
}

impl IntoDomain<Message> for MessageDto {
    fn into_domain(self, (): ()) -> Result<Message, ProtocolError> {
        self.into_message(false)
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
        #[serde(default)]
        failed: bool,
    },
    Error {
        message: String,
    },
    Inject {
        messages: Vec<MessageDto>,
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
                failed,
            } => Ok(Self::Tool {
                resource_id: ResourceIdDto::from_domain(resource_id, ())?,
                tool_call_id: tool_call_id.clone(),
                content: content.clone(),
                failed: *failed,
            }),
            Message::Error { message } => Ok(Self::Error {
                message: message.clone(),
            }),
            Message::Inject { messages } => Ok(Self::Inject {
                messages: messages
                    .iter()
                    .map(|inner| inner.into_dto(()))
                    .collect::<Result<Vec<_>, _>>()?,
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
    #[serde(default)]
    pub name: Option<String>,
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
            name: self.name,
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
pub struct MclCommandDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub command: String,
    pub binding: Option<serde_json::Value>,
}

impl
    IntoDomain<
        RouteMclCommand,
        (
            String,
            String,
            std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
        ),
    > for MclCommandDto
{
    fn into_domain(
        self,
        (id, source, reply): (
            String,
            String,
            std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
        ),
    ) -> Result<RouteMclCommand, ProtocolError> {
        if self.command.trim().is_empty() {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "MCL command is empty",
            ));
        }
        Ok(RouteMclCommand {
            id,
            source,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent.map(|agent| agent.into_domain(())).transpose()?,
            command: self.command,
            binding: self.binding,
            reply,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentTargetDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
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
                name: None,
            },
        }
    }

    pub fn register_connection_with_name(
        id: impl Into<String>,
        client_type: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self::ConnectionRegister {
            id: id.into(),
            message: RegisterConnectionDto {
                client_type: client_type.into(),
                name: Some(name.into()),
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

    pub fn mcl_command(
        id: impl Into<String>,
        workspace: &WorkspaceReferenceDto,
        agent: Option<ResourceIdDto>,
        command: impl Into<String>,
        binding: Option<serde_json::Value>,
    ) -> Self {
        Self::MclCommand {
            id: id.into(),
            message: MclCommandDto {
                workspace: workspace.clone(),
                agent,
                command: command.into(),
                binding,
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
        let id = ResourceRef::new("workspace", "local", &name, None::<String>)
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
pub struct ClientInfoDto {
    pub resource_id: ResourceIdDto,
    pub client_type: String,
    pub name: String,
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
pub struct AgentResourceDto {
    pub resource_id: ResourceIdDto,
    pub resource_name: String,
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
    pub default_visibility_source: Option<BlockPathDto>,
    #[serde(default)]
    pub visibility_source: Option<BlockPathDto>,
    #[serde(default)]
    pub resources: Vec<AgentResourceDto>,
    #[serde(default)]
    pub exposed: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<MessageDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<HistoryRecordDto>,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRecordDto {
    pub kind: String,
    pub content: String,
    pub payload: String,
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
        let dto = match message.message() {
            Some(value) => Self {
                sequence: message.sequence,
                turn_id: message.turn_id(),
                message: Some((&value).into_dto(())?),
                record: None,
                created_at_ms: message.created_at_ms,
            },
            None => Self {
                sequence: message.sequence,
                turn_id: message.turn_id(),
                message: None,
                record: Some(HistoryRecordDto {
                    kind: message.kind.clone(),
                    content: message.content.clone(),
                    payload: message.payload.clone(),
                }),
                created_at_ms: message.created_at_ms,
            },
        };
        Ok(dto)
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

impl FromDomain<&margatroid_types::AgentConversationObserved, &World> for AgentMessageDto {
    fn from_domain(
        observed: &margatroid_types::AgentConversationObserved,
        world: &World,
    ) -> Result<Self, ProtocolError> {
        let (workspace, agent) = agent_route(world, observed.agent)?;
        Ok(Self {
            id: observed.turn_id.clone(),
            workspace,
            agent,
            message: (&observed.message.message).into_dto(())?,
        })
    }
}

impl FromDomain<&AgentFailure, &World> for AgentFailureDto {
    fn from_domain(failure: &AgentFailure, world: &World) -> Result<Self, ProtocolError> {
        let (workspace, agent) = agent_route(world, failure.agent)?;
        let kind = match failure.kind {
            AgentFailureKind::Agent => "agent",
            AgentFailureKind::Inference => "inference",
            AgentFailureKind::Tool => "tool",
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
        let configuration = world.get_component::<Workspace>(workspace).ok_or_else(|| {
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
        let runtime_agent = world.get_component::<Agent>(agent).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::AgentNotFound,
                "Agent MCL state is missing",
            )
        })?;
        let default_resources = runtime_agent
            .resources
            .default_visible
            .iter()
            .map(|resource| resource.into_dto(()))
            .collect::<Result<Vec<_>, _>>()?;
        let visible_resources = runtime_agent
            .resources
            .visible
            .iter()
            .map(|resource| resource.into_dto(()))
            .collect::<Result<Vec<_>, _>>()?;
        let resources = runtime_agent
            .resources
            .tool_entries
            .iter()
            .map(|entry| {
                Ok(AgentResourceDto {
                    resource_id: ResourceIdDto::from_domain(&entry.resource_id, ())?,
                    resource_name: entry.resource_name.clone(),
                })
            })
            .collect::<Result<Vec<_>, ProtocolError>>()?;
        let identity = world.get_component::<ResourceId>(agent).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorKind::AgentNotFound,
                "Agent resource id is missing",
            )
        })?;
        let default_visibility_source = runtime_agent
            .resources
            .default_visible_source
            .as_ref()
            .map(|source| source.into_dto(()))
            .transpose()?;
        let visibility_source = runtime_agent
            .resources
            .visible_source
            .as_ref()
            .map(|source| source.into_dto(()))
            .transpose()?;
        let working = runtime_agent.turn.turn_id.is_some();
        let token_usage = &runtime_agent.token_usage;
        Ok(Self {
            workspace: workspace.reference(),
            agent: identity.as_ref().into_dto(())?,
            status: WorkspaceAgentStatusDto::Ready,
            working,
            error: None,
            default_resources,
            visible_resources,
            default_visibility_source,
            visibility_source,
            resources,
            exposed: runtime_agent.resources.exposed.clone(),
            mcl: None,
            total_input_tokens: token_usage.total_input_tokens,
            total_output_tokens: token_usage.total_output_tokens,
            total_cache_hit_tokens: token_usage.total_cache_hit_tokens,
            cache_hit_rate: token_usage.cache_hit_rate,
            last_input_tokens: token_usage.last_input_tokens,
            context_window_tokens: runtime_agent.info.model.context_window_tokens,
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
            let agents = world.get_component::<Workspace>(workspace).ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorKind::WorkspaceNotFound,
                    "workspace Agent index is missing",
                )
            })?;
            let configuration = world.get_component::<Workspace>(workspace).ok_or_else(|| {
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
                        default_visibility_source: None,
                        visibility_source: None,
                        resources: Vec::new(),
                        exposed: BTreeMap::new(),
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
                        default_visibility_source: None,
                        visibility_source: None,
                        resources: Vec::new(),
                        exposed: BTreeMap::new(),
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
                let memory = world.get_component::<Agent>(agent).ok_or_else(|| {
                    ProtocolError::new(ProtocolErrorKind::MemoryNotFound, "Agent memory is missing")
                })?;
                let messages = memory
                    .memory
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
                        .get_component::<ResourceId>(agent)
                        .ok_or_else(|| {
                            ProtocolError::new(
                                ProtocolErrorKind::AgentNotFound,
                                "Agent identity is missing",
                            )
                        })?
                        .as_ref()
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

impl FromDomain<&ResourceRef> for ResourceIdDto {
    fn from_domain(resource: &ResourceRef, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(resource.to_string()))
    }
}

impl FromDomain<ResourceRef> for ResourceIdDto {
    fn from_domain(resource: ResourceRef, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(resource.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockPathDto {
    pub block_id: String,
    pub inner_id: String,
}

impl FromDomain<&margatroid_types::BlockPath, ()> for BlockPathDto {
    fn from_domain(path: &margatroid_types::BlockPath, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            block_id: path.block_id.clone(),
            inner_id: path.inner_id.clone(),
        })
    }
}

impl IntoDomain<ResourceRef> for ResourceIdDto {
    fn into_domain(self, (): ()) -> Result<ResourceRef, ProtocolError> {
        ResourceRef::parse(self.0).map_err(|error| {
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
    let identity = world.get_component::<ResourceId>(agent).ok_or_else(|| {
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
    let configuration = world.get_component::<Workspace>(workspace).ok_or_else(|| {
        ProtocolError::new(
            ProtocolErrorKind::WorkspaceNotFound,
            "workspace configuration is missing",
        )
    })?;
    let workspace = configuration.definition().into_dto(())?;
    Ok((workspace, identity.as_ref().into_dto(())?))
}

fn agent_resource_id(workspace: &str, agent: &str) -> Result<ResourceRef, ProtocolError> {
    ResourceRef::new("agent", workspace, agent, None::<String>).map_err(|error| {
        ProtocolError::new(
            ProtocolErrorKind::InvalidResourceReference,
            format!("invalid Agent resource ID: {error}"),
        )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolErrorKind {
    HistoryEntryInvalid,
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
