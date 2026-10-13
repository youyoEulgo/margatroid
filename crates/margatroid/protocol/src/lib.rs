use core_plugin::{Entity, World};
use log_plugin::{TracingField, TracingRecord};
use margatroid_types::{
    agent_components::{Agent, AgentState, WorldAgentExt},
    block_types::BlockInner,
    history_components::HistoryRecord,
    mcl_events::{MclCommand, MclCommandResult, MclInstruction, MclValue},
    message_types::{AgentMessage, TokenUsage, ToolCall},
    resource_components::{
        skill_input_schema, CallableDefinitionSnapshot, CallableResource, SandboxValue,
    },
    workspace_components::{
        Workspace, WorkspaceAgentDefinition, WorkspaceDefinition, WorldWorkspaceExt,
    },
    workspace_events::StartWorkspace,
};
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};
use std::{fmt, path::PathBuf};

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
    #[serde(rename = "state.query")]
    StateQuery {
        id: String,
        #[serde(default)]
        message: EmptyQueryDto,
    },
    #[serde(rename = "history.query")]
    HistoryQuery {
        id: String,
        message: RouteAgentTargetDto,
    },
}
impl ClientMessage {
    pub fn id(&self) -> &str {
        match self {
            Self::ConnectionRegister { id, .. }
            | Self::WorkspaceStart { id, .. }
            | Self::WorkspaceStop { id, .. }
            | Self::MclCommand { id, .. }
            | Self::StateQuery { id, .. }
            | Self::HistoryQuery { id, .. } => id,
        }
    }
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
                definition: definition.into_dto(()).expect("valid workspace definition"),
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
        arguments: Vec<serde_json::Value>,
    ) -> Self {
        Self::MclCommand {
            id: id.into(),
            message: MclCommandDto {
                workspace: workspace.clone(),
                agent,
                command: command.into(),
                arguments,
            },
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyQueryDto {}

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
    ConnectionRegisterFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "workspace.started")]
    WorkspaceStarted {
        id: String,
        workspace: WorkspaceInfoDto,
    },
    #[serde(rename = "workspace.start_failed")]
    WorkspaceStartFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "workspace.stopped")]
    WorkspaceStopped {
        id: String,
        workspace: WorkspaceReferenceDto,
    },
    #[serde(rename = "workspace.stop_failed")]
    WorkspaceStopFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "mcl.command_result")]
    MclCommandResult {
        id: String,
        source: ResourceIdDto,
        target: ResourceIdDto,
        result: Result<serde_json::Value, ProtocolErrorDto>,
        history_error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "history.result")]
    HistoryResult {
        id: String,
        history: AgentHistoryDto,
    },
    #[serde(rename = "request.failed")]
    RequestFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "agent.message.finished")]
    AgentMessageFinished {
        id: String,
        agent: ResourceIdDto,
        agent_instance: String,
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.message.delta")]
    AgentMessageDelta {
        id: String,
        agent: ResourceIdDto,
        agent_instance: String,
        sequence: u64,
        content: String,
    },
    #[serde(rename = "agent.message.reasoning_delta")]
    AgentMessageReasoningDelta {
        id: String,
        agent: ResourceIdDto,
        agent_instance: String,
        sequence: u64,
        content: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorDto {
    pub kind: String,
    pub message: String,
}
impl ProtocolErrorDto {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: bounded(message.into()),
        }
    }
}
impl From<ProtocolError> for ProtocolErrorDto {
    fn from(error: ProtocolError) -> Self {
        Self::new(format!("{:?}", error.kind), error.message)
    }
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
                .collect::<Result<_, _>>()?,
            spans: record.spans,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessageDto {
    System {
        content: String,
    },
    Inject {
        messages: Vec<AgentMessageDto>,
    },
    User {
        content: String,
    },
    Assistant {
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
    },
    ToolCall {
        resource_id: Option<ResourceIdDto>,
        tool_call_id: String,
        tool_name: String,
        content: String,
        #[serde(default)]
        failed: bool,
    },
    Error {
        message: String,
    },
}
pub type MessageDto = AgentMessageDto;
pub type ToolCallDto = ToolCall;
impl AgentMessageDto {
    fn convert(self, depth: usize, nodes: &mut usize) -> Result<AgentMessage, ProtocolError> {
        *nodes += 1;
        if depth > 32 || *nodes > 4096 {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "message nesting or count limit exceeded",
            ));
        }
        Ok(match self {
            Self::Inject { messages } => AgentMessage::Inject {
                messages: messages
                    .into_iter()
                    .map(|message| message.convert(depth + 1, nodes))
                    .collect::<Result<_, _>>()?,
            },
            Self::System { content } => AgentMessage::System { content },
            Self::User { content } => AgentMessage::User { content },
            Self::Assistant {
                reasoning,
                content,
                tool_calls,
                usage,
            } => AgentMessage::Assistant {
                reasoning,
                content,
                tool_calls,
                usage,
            },
            Self::ToolCall {
                resource_id,
                tool_call_id,
                tool_name,
                content,
                failed,
            } => AgentMessage::ToolCall {
                resource_id: resource_id.map(|id| id.into_domain(())).transpose()?,
                tool_call_id,
                tool_name,
                content,
                failed,
            },
            Self::Error { message } => AgentMessage::Error { message },
        })
    }
}
impl IntoDomain<AgentMessage> for AgentMessageDto {
    fn into_domain(self, (): ()) -> Result<AgentMessage, ProtocolError> {
        if serde_json::to_vec(&self)
            .map_err(|e| ProtocolError::new(ProtocolErrorKind::InvalidRequest, e.to_string()))?
            .len()
            > 1024 * 1024
        {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "message size limit exceeded",
            ));
        }
        self.convert(0, &mut 0)
    }
}
impl FromDomain<&AgentMessage> for AgentMessageDto {
    fn from_domain(message: &AgentMessage, (): ()) -> Result<Self, ProtocolError> {
        Ok(match message {
            AgentMessage::Inject { messages } => Self::Inject {
                messages: messages
                    .iter()
                    .map(|message| message.into_dto(()))
                    .collect::<Result<_, _>>()?,
            },
            AgentMessage::System { content } => Self::System {
                content: content.clone(),
            },
            AgentMessage::User { content } => Self::User {
                content: content.clone(),
            },
            AgentMessage::Assistant {
                reasoning,
                content,
                tool_calls,
                usage,
            } => Self::Assistant {
                reasoning: reasoning.clone(),
                content: content.clone(),
                tool_calls: tool_calls.clone(),
                usage: usage.clone(),
            },
            AgentMessage::ToolCall {
                resource_id,
                tool_call_id,
                tool_name,
                content,
                failed,
            } => Self::ToolCall {
                resource_id: resource_id.as_ref().map(|id| id.into_dto(())).transpose()?,
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: content.clone(),
                failed: *failed,
            },
            AgentMessage::Error { message } => Self::Error {
                message: message.clone(),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceIdDto(pub String);
pub use ResourceIdDto as ResourceIDDto;
impl fmt::Display for ResourceIdDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl FromDomain<&ResourceID> for ResourceIdDto {
    fn from_domain(id: &ResourceID, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(id.to_string()))
    }
}
impl FromDomain<ResourceID> for ResourceIdDto {
    fn from_domain(id: ResourceID, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self(id.to_string()))
    }
}
impl IntoDomain<ResourceID> for ResourceIdDto {
    fn into_domain(self, (): ()) -> Result<ResourceID, ProtocolError> {
        ResourceID::parse(self.0).map_err(|e| {
            ProtocolError::new(ProtocolErrorKind::InvalidResourceReference, e.to_string())
        })
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
        valid_id(&id)?;
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
        valid_id(&id)?;
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentTargetDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclCommandDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: Option<ResourceIdDto>,
    pub command: String,
    #[serde(default)]
    pub arguments: Vec<serde_json::Value>,
}
impl IntoDomain<MclCommand, (String, ResourceID, ResourceID)> for MclCommandDto {
    fn into_domain(
        self,
        (id, source, target): (String, ResourceID, ResourceID),
    ) -> Result<MclCommand, ProtocolError> {
        valid_id(&id)?;
        if self.command.trim().is_empty()
            || self.command.len() > 65536
            || self.arguments.iter().any(|value| !value.is_array())
        {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "MCL command or array arguments invalid",
            ));
        }
        Ok(MclCommand {
            id,
            source,
            target,
            command: MclInstruction {
                text: self.command,
                arguments: self.arguments,
            },
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallableResourceDto {
    pub resource_id: ResourceIdDto,
    pub alias: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
impl FromDomain<&CallableResource> for CallableResourceDto {
    fn from_domain(value: &CallableResource, (): ()) -> Result<Self, ProtocolError> {
        let (description, input_schema) = match value.definition.as_ref() {
            CallableDefinitionSnapshot::Tool(definition) => (
                definition.description.clone(),
                definition.input_schema.clone(),
            ),
            CallableDefinitionSnapshot::Hook(definition) => (
                definition.description.clone(),
                definition.input_schema.clone(),
            ),
            CallableDefinitionSnapshot::Skill(definition) => {
                (definition.description.clone(), skill_input_schema())
            }
        };
        Ok(Self {
            resource_id: (&value.resource_id).into_dto(())?,
            alias: value.alias.clone(),
            description,
            input_schema,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SandboxValueDto {
    pub resource_id: ResourceIdDto,
}
impl FromDomain<&SandboxValue> for SandboxValueDto {
    fn from_domain(value: &SandboxValue, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            resource_id: (&value.resource_id).into_dto(())?,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum BlockInnerDto {
    Message(Vec<AgentMessageDto>),
    Tool(Vec<CallableResourceDto>),
    Sandbox(Vec<SandboxValueDto>),
}
impl FromDomain<&BlockInner> for BlockInnerDto {
    fn from_domain(value: &BlockInner, (): ()) -> Result<Self, ProtocolError> {
        Ok(match value {
            BlockInner::Message(values) => Self::Message(
                values
                    .iter()
                    .map(|value| value.into_dto(()))
                    .collect::<Result<_, _>>()?,
            ),
            BlockInner::Tool(values) => Self::Tool(
                values
                    .iter()
                    .map(|value| value.into_dto(()))
                    .collect::<Result<_, _>>()?,
            ),
            BlockInner::Sandbox(values) => Self::Sandbox(
                values
                    .iter()
                    .map(|value| value.into_dto(()))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}
fn project_mcl_value(value: &MclValue) -> Result<serde_json::Value, ProtocolError> {
    let projected = match value {
        MclValue::Array(array) => {
            serde_json::json!({ "Array": BlockInnerDto::from_domain(array, ())? })
        }
        _ => serde_json::to_value(value).map_err(|error| {
            ProtocolError::new(ProtocolErrorKind::InvalidRequest, error.to_string())
        })?,
    };
    Ok(projected)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclCommandResultDto {
    pub id: String,
    pub source: ResourceIdDto,
    pub target: ResourceIdDto,
    pub result: Result<serde_json::Value, ProtocolErrorDto>,
    pub history_error: Option<ProtocolErrorDto>,
}
impl FromDomain<&MclCommandResult> for MclCommandResultDto {
    fn from_domain(event: &MclCommandResult, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: event.id.clone(),
            source: (&event.source).into_dto(())?,
            target: (&event.target).into_dto(())?,
            result: match &event.result {
                Ok(value) => Ok(project_mcl_value(value)?),
                Err(error) => Err(ProtocolErrorDto::new("MclError", error.to_string())),
            },
            history_error: event
                .history_error
                .as_ref()
                .map(|error| ProtocolErrorDto::new("HistoryWriteFailed", error.to_string())),
        })
    }
}
impl From<MclCommandResultDto> for ServerMessage {
    fn from(dto: MclCommandResultDto) -> Self {
        Self::MclCommandResult {
            id: dto.id,
            source: dto.source,
            target: dto.target,
            result: dto.result,
            history_error: dto.history_error,
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
        let id = ResourceID::new("workspace", "local", &name, None).expect("valid workspace name");
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
            &definition.name,
            definition.project_root.to_string_lossy(),
        ))
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub id: ResourceIdDto,
    pub image: ResourceIdDto,
    pub home_root: String,
    pub history_path: String,
}
impl FromDomain<&WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn from_domain(agent: &WorkspaceAgentDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: agent.name.clone(),
            id: (&agent.id).into_dto(())?,
            image: (&agent.image).into_dto(())?,
            home_root: agent.home_root.to_string_lossy().into(),
            history_path: agent.history_path.to_string_lossy().into(),
        })
    }
}
impl IntoDomain<WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceAgentDefinition, ProtocolError> {
        let id: ResourceID = self.id.into_domain(())?;
        let image: ResourceID = self.image.into_domain(())?;
        if id.resource_type() != "agent"
            || image.resource_type() != "image"
            || self.home_root.is_empty()
            || self.history_path.is_empty()
        {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "invalid agent definition",
            ));
        }
        Ok(WorkspaceAgentDefinition {
            name: self.name,
            id,
            image,
            home_root: self.home_root.into(),
            history_path: self.history_path.into(),
        })
    }
}
impl FromDomain<&WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(|agent| agent.into_dto(()))
                .collect::<Result<_, _>>()?,
        })
    }
}
impl IntoDomain<WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceDefinition, ProtocolError> {
        let id: ResourceID = self.id.into_domain(())?;
        if id.resource_type() != "workspace" || self.project_root.is_empty() {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "invalid workspace definition",
            ));
        }
        Ok(WorkspaceDefinition {
            id,
            name: self.name,
            project_root: self.project_root.into(),
            manager: self.manager,
            agents: self
                .agents
                .into_iter()
                .map(|agent| agent.into_domain(()))
                .collect::<Result<_, _>>()?,
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
impl WorkspaceInfoDto {
    pub fn reference(&self) -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::with_id(self.id.clone(), &self.name, &self.project_root)
    }
}
impl FromDomain<&WorkspaceDefinition> for WorkspaceInfoDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        let manager = definition
            .agents
            .iter()
            .find(|agent| agent.name == definition.manager)
            .ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorKind::AgentNotFound,
                    "manager definition missing",
                )
            })?;
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into(),
            manager: (&manager.id).into_dto(())?,
            agents: definition
                .agents
                .iter()
                .map(|agent| (&agent.id).into_dto(()))
                .collect::<Result<_, _>>()?,
        })
    }
}
impl FromDomain<Entity, &World> for WorkspaceInfoDto {
    fn from_domain(entity: Entity, world: &World) -> Result<Self, ProtocolError> {
        world
            .get_component::<Workspace>(entity)
            .ok_or_else(|| {
                ProtocolError::new(ProtocolErrorKind::WorkspaceNotFound, "workspace missing")
            })?
            .definition()
            .into_dto(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfoDto {
    pub resource_id: ResourceIdDto,
    pub client_type: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStateDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub agent_instance: String,
    pub status: String,
    pub turn: Option<String>,
    pub sandbox: Option<ResourceIdDto>,
    pub queued_commands: usize,
    pub pending_command: Option<String>,
}
impl FromDomain<Entity, &World> for AgentStateDto {
    fn from_domain(entity: Entity, world: &World) -> Result<Self, ProtocolError> {
        let agent = world
            .get_component::<Agent>(entity)
            .ok_or_else(|| ProtocolError::new(ProtocolErrorKind::AgentNotFound, "agent missing"))?;
        let workspace = world
            .workspace_by_id(&agent.workspace)
            .map_err(|e| ProtocolError::new(ProtocolErrorKind::WorkspaceNotFound, e.to_string()))?;
        Ok(Self {
            workspace: world
                .get_component::<Workspace>(workspace)
                .unwrap()
                .definition()
                .into_dto(())?,
            agent: (&agent.resourceid).into_dto(())?,
            agent_instance: format!("{entity:?}"),
            status: match agent.state {
                AgentState::Creating { .. } => "creating",
                AgentState::Running => "running",
                AgentState::Stopping { .. } => "stopping",
            }
            .into(),
            turn: agent.turn.clone(),
            sandbox: agent
                .sandbox
                .as_ref()
                .map(|value| (&value.resource_id).into_dto(()))
                .transpose()?,
            queued_commands: agent.mcl.commands.len(),
            pending_command: agent
                .mcl
                .pending
                .as_ref()
                .map(|pending| pending.command_id.clone()),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessageDto {
    pub key: String,
    pub sequence: i64,
    pub turn: Option<String>,
    pub kind: String,
    pub content: String,
    pub payload: serde_json::Value,
    pub created_at_ms: i64,
}
impl FromDomain<(usize, HistoryRecord)> for HistoryMessageDto {
    fn from_domain(
        (sequence, record): (usize, HistoryRecord),
        (): (),
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            key: record.key,
            sequence: sequence as i64 + 1,
            turn: record.turn,
            kind: record.kind,
            content: record.content,
            payload: record.payload,
            created_at_ms: 0,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHistoryDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub messages: Vec<HistoryMessageDto>,
}
impl FromDomain<(AgentStateDto, Vec<HistoryRecord>)> for AgentHistoryDto {
    fn from_domain(
        (state, records): (AgentStateDto, Vec<HistoryRecord>),
        (): (),
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            workspace: state.workspace,
            agent: state.agent,
            messages: records
                .into_iter()
                .enumerate()
                .map(|record| record.into_dto(()))
                .collect::<Result<_, _>>()?,
        })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStateDto {
    pub workspaces: Vec<WorkspaceInfoDto>,
    pub agents: Vec<AgentStateDto>,
    pub histories: Vec<AgentHistoryDto>,
}
impl FromDomain<(), &World> for BackendStateDto {
    fn from_domain((): (), world: &World) -> Result<Self, ProtocolError> {
        let mut workspaces: Vec<WorkspaceInfoDto> = world
            .workspace_list()
            .into_iter()
            .map(|entity| entity.into_dto(world))
            .collect::<Result<_, _>>()?;
        workspaces.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let mut entities = world.query_with::<Agent>().result();
        entities.sort_by_key(|entity| {
            world
                .get_component::<Agent>(*entity)
                .unwrap()
                .resourceid
                .to_string()
        });
        Ok(Self {
            workspaces,
            agents: entities
                .iter()
                .map(|entity| (*entity).into_dto(world))
                .collect::<Result<_, _>>()?,
            histories: Vec::new(),
        })
    }
}

pub fn resolve_workspace(
    world: &World,
    reference: &WorkspaceReferenceDto,
) -> Result<Entity, ProtocolError> {
    let id: ResourceID = reference.id.clone().into_domain(())?;
    let entity = world
        .workspace_by_id(&id)
        .map_err(|e| ProtocolError::new(ProtocolErrorKind::WorkspaceNotFound, e.to_string()))?;
    let definition = world
        .get_component::<Workspace>(entity)
        .unwrap()
        .definition();
    if definition.name != reference.name
        || definition.project_root != PathBuf::from(&reference.project_root)
    {
        return Err(ProtocolError::new(
            ProtocolErrorKind::WorkspaceNotFound,
            "workspace reference metadata does not match",
        ));
    }
    Ok(entity)
}
pub fn resolve_agent(
    world: &World,
    reference: &WorkspaceReferenceDto,
    requested: Option<&ResourceIdDto>,
) -> Result<(Entity, ResourceID), ProtocolError> {
    let workspace = resolve_workspace(world, reference)?;
    let component = world.get_component::<Workspace>(workspace).unwrap();
    let id = match requested {
        Some(id) => id.clone().into_domain(())?,
        None => component
            .definition()
            .agents
            .iter()
            .find(|agent| agent.name == component.definition().manager)
            .ok_or_else(|| ProtocolError::new(ProtocolErrorKind::AgentNotFound, "manager missing"))?
            .id
            .clone(),
    };
    let entity = world
        .agent_by_id(&id)
        .map_err(|e| ProtocolError::new(ProtocolErrorKind::AgentNotFound, e.to_string()))?;
    let agent = world.get_component::<Agent>(entity).unwrap();
    if agent.workspace != component.definition().id
        || !component
            .iter()
            .any(|(member, current)| member == &id && current == entity)
    {
        return Err(ProtocolError::new(
            ProtocolErrorKind::AgentNotFound,
            "agent is not a current workspace member",
        ));
    }
    Ok((entity, id))
}
fn valid_id(id: &str) -> Result<(), ProtocolError> {
    if id.trim().is_empty() || id.len() > 512 {
        Err(ProtocolError::new(
            ProtocolErrorKind::InvalidRequest,
            "invalid request ID",
        ))
    } else {
        Ok(())
    }
}
fn bounded(mut message: String) -> String {
    if message.len() > 512 {
        let mut boundary = 509;
        while !message.is_char_boundary(boundary) {
            boundary -= 1;
        }
        message.truncate(boundary);
        message.push_str("...");
    }
    message
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolErrorKind {
    InvalidRequest,
    InvalidResourceReference,
    AgentNotFound,
    WorkspaceNotFound,
    HistoryReadFailed,
    Unregistered,
    DuplicateRequest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    kind: ProtocolErrorKind,
    message: String,
}
impl ProtocolError {
    pub fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: bounded(message.into()),
        }
    }
    pub fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}:{}", self.kind, self.message)
    }
}
impl std::error::Error for ProtocolError {}
