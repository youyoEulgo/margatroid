use std::fmt;
use std::path::PathBuf;

use agent_plugin::{AgentDynamicVisibility, AgentIdentity};
use core_plugin::{Entity, World};
use log_plugin::{TracingField, TracingRecord};
use margatroid_types::{
    AgentFailure, AgentFailureKind, AgentImageReference, AgentMessage, Message, ResourceName,
    ResourceRef, RouteAgentMessage, StartWorkspace, ToolCall, WorkspaceAgentDefinition,
    WorkspaceDefinition, WorkspaceReference,
};
use memory_plugin::{AgentMemory, HistoryMessage};
use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};
use workspace_plugin::{
    StopWorkspaceByReference, WorkspaceAgents, WorkspaceConfiguration, WorldWorkspaceExt,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        agent: String,
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
pub struct UserMessageDto {
    pub content: String,
}

impl IntoDomain<Message> for UserMessageDto {
    fn into_domain(self, (): ()) -> Result<Message, ProtocolError> {
        Ok(Message::User {
            content: self.content,
            tool_calls: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallDto {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl FromDomain<&ToolCall> for ToolCallDto {
    fn from_domain(call: &ToolCall, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        })
    }
}

impl IntoDomain<ToolCall> for ToolCallDto {
    fn into_domain(self, (): ()) -> Result<ToolCall, ProtocolError> {
        Ok(ToolCall {
            id: self.id,
            name: self.name,
            arguments: self.arguments,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDto {
    User {
        content: String,
        tool_calls: Vec<ToolCallDto>,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCallDto>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl FromDomain<&Message> for MessageDto {
    fn from_domain(message: &Message, (): ()) -> Result<Self, ProtocolError> {
        match message {
            Message::User {
                content,
                tool_calls,
            } => Ok(Self::User {
                content: content.clone(),
                tool_calls: tool_calls
                    .iter()
                    .map(|call| call.into_dto(()))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Message::Assistant {
                content,
                tool_calls,
            } => Ok(Self::Assistant {
                content: content.clone(),
                tool_calls: tool_calls
                    .iter()
                    .map(|call| call.into_dto(()))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Message::Tool {
                tool_call_id,
                content,
            } => Ok(Self::Tool {
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
    pub agent: Option<String>,
    pub message: UserMessageDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallDto>,
}

impl IntoDomain<RouteAgentMessage, String> for RouteAgentMessageDto {
    fn into_domain(self, id: String) -> Result<RouteAgentMessage, ProtocolError> {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .map(|call| call.into_domain(()))
            .collect::<Result<_, _>>()?;
        Ok(RouteAgentMessage {
            id,
            workspace: self.workspace.into_domain(())?,
            agent: self.agent,
            message: Message::User {
                content: self.message.content,
                tool_calls,
            },
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
        agent: Option<String>,
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
                tool_calls: Vec::new(),
            },
        }
    }

    pub fn agent_message_with_tool_calls(
        id: impl Into<String>,
        workspace: &WorkspaceReferenceDto,
        agent: Option<String>,
        content: impl Into<String>,
        tool_calls: Vec<ToolCallDto>,
    ) -> Self {
        Self::AgentMessage {
            id: id.into(),
            message: RouteAgentMessageDto {
                workspace: workspace.clone(),
                agent,
                message: UserMessageDto {
                    content: content.into(),
                },
                tool_calls,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceReferenceDto {
    pub name: String,
    pub project_root: String,
}

impl WorkspaceReferenceDto {
    pub fn new(name: impl Into<String>, project_root: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            project_root: project_root.into(),
        }
    }
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceReferenceDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self::new(
            definition.name.clone(),
            definition.project_root.to_string_lossy().into_owned(),
        ))
    }
}

impl FromDomain<WorkspaceReference> for WorkspaceReferenceDto {
    fn from_domain(reference: WorkspaceReference, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self::new(
            reference.name,
            reference.project_root.to_string_lossy().into_owned(),
        ))
    }
}

impl IntoDomain<WorkspaceReference> for WorkspaceReferenceDto {
    fn into_domain(self, (): ()) -> Result<WorkspaceReference, ProtocolError> {
        Ok(WorkspaceReference {
            name: self.name,
            project_root: PathBuf::from(self.project_root),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    pub name: String,
    pub project_root: String,
    pub manager: String,
    pub agents: Vec<String>,
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceInfoDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into_owned(),
            manager: definition.manager.clone(),
            agents: definition
                .agents
                .iter()
                .map(|agent| agent.name.clone())
                .collect(),
        })
    }
}

impl WorkspaceInfoDto {
    pub fn reference(&self) -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::new(self.name.clone(), self.project_root.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStateDto {
    pub workspaces: Vec<WorkspaceInfoDto>,
    #[serde(default)]
    pub agents: Vec<AgentStateDto>,
    #[serde(default)]
    pub histories: Vec<AgentHistoryDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStateDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: String,
    pub visible_resources: Vec<ResourceRefDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHistoryDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: String,
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
    pub agent: String,
    pub message: MessageDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFailureDto {
    pub id: String,
    pub workspace: WorkspaceReferenceDto,
    pub agent: String,
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
        (agent, name, workspace): (Entity, &str, &WorkspaceInfoDto),
        world: &World,
    ) -> Result<Self, ProtocolError> {
        let visibility = world
            .get_component::<AgentDynamicVisibility>(agent)
            .ok_or_else(|| {
                ProtocolError::new(
                    ProtocolErrorKind::AgentNotFound,
                    "Agent dynamic visibility is missing",
                )
            })?;
        let visible_resources = visibility
            .resources()
            .iter()
            .map(|resource| resource.into_dto(()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            workspace: workspace.reference(),
            agent: name.to_owned(),
            visible_resources,
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
            for (name, agent) in agents.iter() {
                agent_states.push(AgentStateDto::from_domain((agent, name, &info), world)?);
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
                    agent: name.to_owned(),
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
    pub name: String,
    pub project_root: String,
    pub manager: String,
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}

impl FromDomain<&WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
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
    pub image: String,
    pub resources: Vec<ResourceRefDto>,
    pub disable_resources: Vec<ResourceRefDto>,
    pub memory_path: Option<String>,
}

impl FromDomain<&WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn from_domain(definition: &WorkspaceAgentDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: definition.name.clone(),
            image: definition.image.to_string(),
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
        let image = AgentImageReference::new(self.image).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidImageReference,
                format!("invalid AgentImage reference: {error}"),
            )
        })?;
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
            image,
            resources,
            disable_resources,
            memory_path: self.memory_path.map(PathBuf::from),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRefDto {
    pub provider: String,
    pub name: String,
}

impl FromDomain<&ResourceRef> for ResourceRefDto {
    fn from_domain(resource: &ResourceRef, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            provider: resource.provider().to_owned(),
            name: resource.name().to_string(),
        })
    }
}

impl IntoDomain<ResourceRef> for ResourceRefDto {
    fn into_domain(self, (): ()) -> Result<ResourceRef, ProtocolError> {
        let name = ResourceName::new(self.name).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidResourceReference,
                format!("invalid resource name: {error}"),
            )
        })?;
        ResourceRef::new(self.provider, name).map_err(|error| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidResourceReference,
                format!("invalid resource reference: {error}"),
            )
        })
    }
}

fn agent_route(
    world: &World,
    agent: Entity,
) -> Result<(WorkspaceReferenceDto, String), ProtocolError> {
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
    Ok((workspace, identity.id().to_owned()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolErrorKind {
    AgentNotFound,
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
    use std::collections::BTreeSet;
    use std::path::Path;

    use agent_plugin::{AgentCreateRequest, AgentCreated, AgentPlugin};
    use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
    use core_plugin::App;

    use super::*;

    fn definition() -> WorkspaceDefinition {
        WorkspaceDefinition {
            name: "demo".into(),
            project_root: Path::new("/tmp/demo").into(),
            manager: "coder".into(),
            agents: vec![WorkspaceAgentDefinition {
                name: "coder".into(),
                image: AgentImageReference::new("local/coder:v1").unwrap(),
                resources: vec![ResourceRef::new(
                    "skill",
                    ResourceName::new("local/context").unwrap(),
                )
                .unwrap()],
                disable_resources: Vec::new(),
                memory_path: None,
            }],
        }
    }

    #[test]
    fn request_uses_stable_workspace_start_shape() {
        let value =
            serde_json::to_value(ClientMessage::start_workspace("request-1", &definition()))
                .unwrap();
        assert_eq!(value["type"], "workspace.start");
        assert_eq!(value["id"], "request-1");
        assert_eq!(
            value["message"]["definition"]["agents"][0]["image"],
            "local/coder:v1"
        );
        assert_eq!(
            value["message"]["definition"]["agents"][0]["resources"][0]["name"],
            "local/context"
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
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
        let value =
            serde_json::to_value(ClientMessage::stop_workspace("stop-1", &workspace)).unwrap();

        assert_eq!(value["type"], "workspace.stop");
        assert_eq!(value["id"], "stop-1");
        assert_eq!(value["message"]["workspace"]["name"], "demo");
        assert_eq!(value["message"]["workspace"]["project_root"], "/tmp/demo");
    }

    #[test]
    fn workspace_stop_result_uses_stable_server_shapes() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
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
    fn agent_message_uses_workspace_identity_and_optional_agent_name() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
        let value = serde_json::to_value(ClientMessage::agent_message(
            "message-1",
            &workspace,
            Some("reviewer".into()),
            "Review this change.",
        ))
        .unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["workspace"]["name"], "demo");
        assert_eq!(value["message"]["workspace"]["project_root"], "/tmp/demo");
        assert_eq!(value["message"]["agent"], "reviewer");
        assert_eq!(
            value["message"]["message"]["content"],
            "Review this change."
        );
    }

    #[test]
    fn agent_message_preserves_preselected_tool_calls() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
        let request = ClientMessage::agent_message_with_tool_calls(
            "message-1",
            &workspace,
            None,
            "Load context.",
            vec![ToolCallDto {
                id: "call-1".into(),
                name: "skill_local_context".into(),
                arguments: "{}".into(),
            }],
        );
        let ClientMessage::AgentMessage { id, message } = request else {
            panic!("agent message constructor returned a different request type");
        };
        let routed = message.into_domain(id).unwrap();

        let Message::User { tool_calls, .. } = routed.message else {
            panic!("route did not contain a user message");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "skill_local_context");
    }

    #[test]
    fn omitted_agent_is_encoded_for_manager_fallback() {
        let workspace = WorkspaceReferenceDto::new("demo", "/tmp/demo");
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
        assert_eq!(value["workspace"]["manager"], "coder");
        assert_eq!(value["workspace"]["agents"][0], "coder");
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
                    workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
                    agent: "coder".into(),
                    visible_resources: vec![ResourceRefDto {
                        provider: "skill".into(),
                        name: "local/review".into(),
                    }],
                }],
                histories: vec![AgentHistoryDto {
                    workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
                    agent: "coder".into(),
                    messages: vec![HistoryMessageDto {
                        sequence: 1,
                        turn_id: "turn-1".into(),
                        message: MessageDto::User {
                            content: "hello".into(),
                            tool_calls: Vec::new(),
                        },
                        created_at_ms: 42,
                    }],
                }],
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "state.sync");
        assert_eq!(value["state"]["workspaces"][0]["name"], "demo");
        assert_eq!(value["state"]["workspaces"][0]["manager"], "coder");
        assert_eq!(value["state"]["agents"][0]["agent"], "coder");
        assert_eq!(
            value["state"]["agents"][0]["visible_resources"][0]["provider"],
            "skill"
        );
        assert_eq!(value["state"]["histories"][0]["agent"], "coder");
        assert_eq!(
            value["state"]["histories"][0]["messages"][0]["turn_id"],
            "turn-1"
        );
    }

    #[test]
    fn agent_state_projects_dynamic_visibility() {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AgentPlugin::default());
        let workspace = app.world_mut().spawn();
        let skill = ResourceRef::new("skill", ResourceName::new("local/review").unwrap()).unwrap();
        app.world().send_event(AgentCreateRequest {
            id: "create-1".into(),
            agent_id: "demo.coder0".into(),
            workspace_id: workspace,
            system_prompt: "system".into(),
            messages: Vec::new(),
            tool_context: Vec::new(),
            default_visibility: BTreeSet::from([skill]),
        });
        app.tick();
        app.tick();
        let agent = app
            .world()
            .event_reader::<AgentCreated>()
            .into_iter()
            .find(|event| event.id == "create-1")
            .unwrap()
            .agent;
        let workspace = (&definition()).into_dto(()).unwrap();

        let state = AgentStateDto::from_domain((agent, "coder", &workspace), app.world()).unwrap();

        assert_eq!(state.workspace.name, "demo");
        assert_eq!(state.agent, "coder");
        assert_eq!(state.visible_resources.len(), 1);
        assert_eq!(state.visible_resources[0].provider, "skill");
        assert_eq!(state.visible_resources[0].name, "local/review");
    }

    #[test]
    fn agent_message_event_exposes_resolved_route_and_message() {
        let event = ServerMessage::AgentMessage {
            message: AgentMessageDto {
                id: "message-1".into(),
                workspace: WorkspaceReferenceDto::new("demo", "/tmp/demo"),
                agent: "coder".into(),
                message: MessageDto::Assistant {
                    content: Some("Done.".into()),
                    tool_calls: Vec::new(),
                },
            },
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message");
        assert_eq!(value["message"]["agent"], "coder");
        assert_eq!(value["message"]["message"]["Assistant"]["content"], "Done.");
    }

    #[test]
    fn agent_message_delta_serializes_as_a_flat_stream_frame() {
        let event = ServerMessage::AgentMessageDelta {
            id: "turn-1".into(),
            agent: "demo.coder0".into(),
            content: "hello".into(),
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "agent.message.delta");
        assert_eq!(value["id"], "turn-1");
        assert_eq!(value["agent"], "demo.coder0");
        assert_eq!(value["content"], "hello");
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
