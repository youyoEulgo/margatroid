use core_plugin::{Entity, World};
use log_plugin::{TracingField, TracingRecord};
use margatroid_types::{
    agent_components::{Agent, AgentFailure, AgentState, WorldAgentExt},
    block_types::BlockInner,
    history_components::HistoryRecord,
    mcl_events::{MclCommand, MclCommandResult, MclInstruction, MclValue},
    message_types::{validate_user_images, AgentMessage, StopReason, TokenUsage, ToolCall},
    resource_components::{CallableDefinitionSnapshot, CallableResource, SandboxValue},
    workspace_catalog::{
        DriverCatalog, WorkspaceCatalog, WorkspaceCatalogEntry, WorkspaceDeletion,
    },
    workspace_components::{
        Workspace, WorkspaceAgentDefinition, WorkspaceDefinition, WorldWorkspaceExt,
    },
    workspace_events::StartRegisteredWorkspace,
};
use resource_id_plugin::ResourceID;
use serde::{Deserialize, Serialize};
use server_plugin::{RegisterConnection, WebSocketConnectionId};
use std::{fmt, path::PathBuf};

pub use margatroid_types::message_types::UserImage;

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
    #[serde(rename = "workspace.paths.query")]
    WorkspacePathsQuery {
        id: String,
        #[serde(default)]
        message: ProjectDirectoriesQueryDto,
    },
    #[serde(rename = "driver.catalog.query")]
    DriverCatalogQuery {
        id: String,
        #[serde(default)]
        message: EmptyQueryDto,
    },
    #[serde(rename = "driver.catalog.refresh")]
    DriverCatalogRefresh {
        id: String,
        #[serde(default)]
        message: EmptyQueryDto,
    },
    #[serde(rename = "workspace.catalog.query")]
    WorkspaceCatalogQuery {
        id: String,
        #[serde(default)]
        message: EmptyQueryDto,
    },
    #[serde(rename = "workspace.catalog.refresh")]
    WorkspaceCatalogRefresh {
        id: String,
        #[serde(default)]
        message: EmptyQueryDto,
    },
    #[serde(rename = "workspace.member.add")]
    WorkspaceMemberAdd {
        id: String,
        message: WorkspaceMemberAddDto,
    },
    #[serde(rename = "workspace.member.remove")]
    WorkspaceMemberRemove {
        id: String,
        message: WorkspaceMemberRemoveDto,
    },
    #[serde(rename = "workspace.create")]
    WorkspaceCreate {
        id: String,
        message: WorkspaceCreateDto,
    },
    #[serde(rename = "workspace.rename")]
    WorkspaceRename {
        id: String,
        message: WorkspaceRenameDto,
    },
    #[serde(rename = "workspace.delete")]
    WorkspaceDelete {
        id: String,
        message: WorkspaceDeleteDto,
    },
    #[serde(rename = "workspace.member.rename")]
    WorkspaceMemberRename {
        id: String,
        message: WorkspaceMemberRenameDto,
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
    #[serde(rename = "agent.turn.abort")]
    AgentTurnAbort {
        id: String,
        message: AbortAgentTurnDto,
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
            Self::WorkspacePathsQuery { id, .. }
            | Self::DriverCatalogQuery { id, .. }
            | Self::DriverCatalogRefresh { id, .. }
            | Self::WorkspaceCatalogQuery { id, .. }
            | Self::WorkspaceCatalogRefresh { id, .. }
            | Self::WorkspaceMemberAdd { id, .. }
            | Self::WorkspaceMemberRemove { id, .. }
            | Self::WorkspaceCreate { id, .. }
            | Self::WorkspaceRename { id, .. }
            | Self::WorkspaceDelete { id, .. }
            | Self::WorkspaceMemberRename { id, .. }
            | Self::ConnectionRegister { id, .. }
            | Self::WorkspaceStart { id, .. }
            | Self::WorkspaceStop { id, .. }
            | Self::AgentTurnAbort { id, .. }
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
    pub fn start_workspace(
        id: impl Into<String>,
        workspace: ResourceIdDto,
        expected_revision: impl Into<String>,
    ) -> Self {
        Self::WorkspaceStart {
            id: id.into(),
            message: StartWorkspaceDto {
                workspace,
                expected_revision: expected_revision.into(),
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
    pub fn create_workspace(
        id: impl Into<String>,
        name: impl Into<String>,
        project_root: impl Into<String>,
    ) -> Self {
        Self::WorkspaceCreate {
            id: id.into(),
            message: WorkspaceCreateDto {
                name: name.into(),
                project_root: project_root.into(),
            },
        }
    }
    pub fn rename_workspace(
        id: impl Into<String>,
        workspace: ResourceIdDto,
        expected_revision: impl Into<String>,
        new_name: impl Into<String>,
    ) -> Self {
        Self::WorkspaceRename {
            id: id.into(),
            message: WorkspaceRenameDto {
                workspace,
                expected_revision: expected_revision.into(),
                new_name: new_name.into(),
            },
        }
    }
    pub fn delete_workspace(
        id: impl Into<String>,
        workspace: ResourceIdDto,
        expected_revision: impl Into<String>,
    ) -> Self {
        Self::WorkspaceDelete {
            id: id.into(),
            message: WorkspaceDeleteDto {
                workspace,
                expected_revision: expected_revision.into(),
            },
        }
    }
    pub fn rename_workspace_member(
        id: impl Into<String>,
        workspace: ResourceIdDto,
        expected_revision: impl Into<String>,
        agent: ResourceIdDto,
        new_name: impl Into<String>,
    ) -> Self {
        Self::WorkspaceMemberRename {
            id: id.into(),
            message: WorkspaceMemberRenameDto {
                workspace,
                expected_revision: expected_revision.into(),
                agent,
                new_name: new_name.into(),
            },
        }
    }
    pub fn mcl_command(
        id: impl Into<String>,
        workspace: &WorkspaceReferenceDto,
        agent: ResourceIdDto,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDirectoriesQueryDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
impl IntoDomain<margatroid_types::discovery::QueryProjectDirectories, String>
    for ProjectDirectoriesQueryDto
{
    fn into_domain(
        self,
        id: String,
    ) -> Result<margatroid_types::discovery::QueryProjectDirectories, ProtocolError> {
        valid_id(&id)?;
        Ok(margatroid_types::discovery::QueryProjectDirectories {
            id,
            path: self.path.map(Into::into),
            cursor: self.cursor,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryEntryDto {
    pub name: String,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryErrorDto {
    pub path: String,
    pub message: String,
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
    ConnectionRegisterFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "workspace.paths")]
    WorkspacePaths {
        id: String,
        path: String,
        parent: Option<String>,
        directories: Vec<ProjectDirectoryEntryDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<String>,
    },
    #[serde(rename = "driver.catalog")]
    DriverCatalog {
        id: String,
        drivers: Vec<ResourceIdDto>,
        errors: Vec<DiscoveryErrorDto>,
    },
    #[serde(rename = "workspace.catalog")]
    WorkspaceCatalog {
        id: String,
        entries: Vec<WorkspaceCatalogEntryDto>,
    },
    #[serde(rename = "workspace.member.result")]
    WorkspaceMemberResult {
        id: String,
        entry: Option<WorkspaceCatalogEntryDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
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
    #[serde(rename = "agent.turn.abort_result")]
    AgentTurnAbortResult {
        id: String,
        workspace: WorkspaceReferenceDto,
        agent: ResourceIdDto,
        turn_id: String,
        result: Result<(), ProtocolErrorDto>,
    },
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<UserImage>,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay: Option<serde_json::Value>,
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
        #[serde(default)]
        stop_reason: Option<StopReason>,
    },
    ToolCall {
        resource_id: Option<ResourceIdDto>,
        tool_call_id: String,
        tool_name: String,
        content: String,
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "margatroid_types::deserialize_user_images"
        )]
        images: Vec<UserImage>,
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
            Self::User { content, images } => {
                validate_user_images(&images).map_err(|message| {
                    ProtocolError::new(ProtocolErrorKind::InvalidRequest, message)
                })?;
                AgentMessage::User { content, images }
            }
            Self::Assistant {
                replay,
                reasoning,
                content,
                tool_calls,
                usage,
                stop_reason,
            } => AgentMessage::Assistant {
                replay,
                reasoning,
                content,
                tool_calls,
                usage,
                stop_reason,
            },
            Self::ToolCall {
                resource_id,
                tool_call_id,
                tool_name,
                content,
                images,
                failed,
            } => {
                validate_user_images(&images).map_err(|message| {
                    ProtocolError::new(ProtocolErrorKind::InvalidRequest, message)
                })?;
                AgentMessage::ToolCall {
                    resource_id: resource_id.map(|id| id.into_domain(())).transpose()?,
                    tool_call_id,
                    tool_name,
                    content,
                    images,
                    failed,
                }
            }
            Self::Error { message } => AgentMessage::Error { message },
        })
    }
}
impl IntoDomain<AgentMessage> for AgentMessageDto {
    fn into_domain(self, (): ()) -> Result<AgentMessage, ProtocolError> {
        let message = self.convert(0, &mut 0)?;
        struct ByteCount(usize);
        impl std::io::Write for ByteCount {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        fn image_bytes(message: &AgentMessage) -> usize {
            match message {
                AgentMessage::User { images, .. } | AgentMessage::ToolCall { images, .. } => {
                    images.iter().map(|image| image.data.len()).sum()
                }
                AgentMessage::Inject { messages } => messages.iter().map(image_bytes).sum(),
                _ => 0,
            }
        }
        let mut count = ByteCount(0);
        serde_json::to_writer(&mut count, &message).map_err(|_| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "message serialization failed",
            )
        })?;
        if count.0.saturating_sub(image_bytes(&message)) > 1024 * 1024 {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidRequest,
                "non-image message size limit exceeded",
            ));
        }
        Ok(message)
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
            AgentMessage::User { content, images } => Self::User {
                content: content.clone(),
                images: images.clone(),
            },
            AgentMessage::Assistant {
                replay,
                reasoning,
                content,
                tool_calls,
                usage,
                stop_reason,
            } => Self::Assistant {
                replay: replay.clone(),
                reasoning: reasoning.clone(),
                content: content.clone(),
                tool_calls: tool_calls.clone(),
                usage: usage.clone(),
                stop_reason: stop_reason.clone(),
            },
            AgentMessage::ToolCall {
                resource_id,
                tool_call_id,
                tool_name,
                content,
                images,
                failed,
            } => Self::ToolCall {
                resource_id: resource_id.as_ref().map(|id| id.into_dto(())).transpose()?,
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: content.clone(),
                images: images.clone(),
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
#[serde(deny_unknown_fields)]
pub struct StartWorkspaceDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
}
impl IntoDomain<StartRegisteredWorkspace, String> for StartWorkspaceDto {
    fn into_domain(self, id: String) -> Result<StartRegisteredWorkspace, ProtocolError> {
        valid_id(&id)?;
        Ok(StartRegisteredWorkspace {
            id,
            workspace_id: self.workspace.into_domain(())?,
            expected_revision: self.expected_revision,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewWorkspaceAgentDto {
    pub name: String,
    pub driver: ResourceIdDto,
    #[serde(default)]
    pub home_root: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberAddDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
    pub agent: NewWorkspaceAgentDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberRemoveDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
    pub agent: ResourceIdDto,
}
/// Creates a registered workspace; the address, definition location and member defaults are
/// derived by the lifecycle from `name`, and `project_root` must be an existing directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateDto {
    pub name: String,
    pub project_root: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRenameDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
    pub new_name: String,
}
/// Removes the registered workspace definition; the lifecycle keeps the directory recoverable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeleteDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberRenameDto {
    pub workspace: ResourceIdDto,
    pub expected_revision: String,
    pub agent: ResourceIdDto,
    pub new_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopWorkspaceDto {
    pub workspace: WorkspaceReferenceDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortAgentTurnDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub turn_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAgentTargetDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MclCommandDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
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
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub id: ResourceIdDto,
    pub driver: ResourceIdDto,
    pub home_root: String,
}
impl FromDomain<&WorkspaceAgentDefinition> for WorkspaceAgentDefinitionDto {
    fn from_domain(agent: &WorkspaceAgentDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            name: agent.name.clone(),
            id: (&agent.id).into_dto(())?,
            driver: (&agent.driver).into_dto(())?,
            home_root: agent.home_root.to_string_lossy().into(),
        })
    }
}
impl FromDomain<&WorkspaceDefinition> for WorkspaceDefinitionDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into(),
            agents: definition
                .agents
                .iter()
                .map(|agent| agent.into_dto(()))
                .collect::<Result<_, _>>()?,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    pub id: ResourceIdDto,
    pub name: String,
    pub project_root: String,
    pub agents: Vec<ResourceIdDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_failures: Vec<WorkspaceMemberFailureDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMemberFailureDto {
    pub agent: ResourceIdDto,
    pub driver: ResourceIdDto,
    pub error: String,
}
impl WorkspaceInfoDto {
    pub fn reference(&self) -> WorkspaceReferenceDto {
        WorkspaceReferenceDto::with_id(self.id.clone(), &self.name, &self.project_root)
    }
}
impl FromDomain<&WorkspaceDefinition> for WorkspaceInfoDto {
    fn from_domain(definition: &WorkspaceDefinition, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&definition.id).into_dto(())?,
            name: definition.name.clone(),
            project_root: definition.project_root.to_string_lossy().into(),
            agents: definition
                .agents
                .iter()
                .map(|agent| (&agent.id).into_dto(()))
                .collect::<Result<_, _>>()?,
            member_failures: Vec::new(),
        })
    }
}
impl FromDomain<Entity, &World> for WorkspaceInfoDto {
    fn from_domain(entity: Entity, world: &World) -> Result<Self, ProtocolError> {
        let workspace = world.get_component::<Workspace>(entity).ok_or_else(|| {
            ProtocolError::new(ProtocolErrorKind::WorkspaceNotFound, "workspace missing")
        })?;
        let mut dto: Self = workspace.definition().into_dto(())?;
        dto.member_failures = workspace
            .member_failures()
            .iter()
            .map(|(agent, failure)| {
                Ok(WorkspaceMemberFailureDto {
                    agent: agent.into_dto(())?,
                    driver: (&failure.driver).into_dto(())?,
                    error: failure.error.to_string(),
                })
            })
            .collect::<Result<_, ProtocolError>>()?;
        Ok(dto)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeletionDto {
    pub id: ResourceIdDto,
    pub name: String,
    /// Registered definition path that no longer exists.
    pub path: String,
    /// Recoverable directory the definition was moved to.
    pub removed_path: String,
}
impl FromDomain<&WorkspaceDeletion> for WorkspaceDeletionDto {
    fn from_domain(deletion: &WorkspaceDeletion, (): ()) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: (&deletion.id).into_dto(())?,
            name: deletion.id.name().to_owned(),
            path: deletion.path.to_string_lossy().into(),
            removed_path: deletion.removed_path.to_string_lossy().into(),
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
pub struct AgentStateDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: ResourceIdDto,
    pub agent_instance: String,
    pub status: String,
    pub turn: Option<String>,
    pub sandbox: Option<ResourceIdDto>,
    pub queued_commands: usize,
    pub pending_command: Option<String>,
    /// Actual model window captured at admission, matching the Lua startup snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolErrorDto>,
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
                AgentState::Idle => "idle",
                AgentState::Working => "working",
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
            context_window_tokens: world
                .get_component::<margatroid_types::model_components::ModelRuntimeInfo>(entity)
                .map(|model| model.context_window_tokens),
            error: world.get_component::<AgentFailure>(entity).map(|failure| {
                ProtocolErrorDto::new(format!("{:?}", failure.0.kind), failure.0.message.clone())
            }),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessageDto {
    pub key: String,
    pub sequence: i64,
    pub turn: Option<String>,
    pub kind: String,
    /// Message text, error text, or MCL command text; Inject envelopes use empty text.
    pub content: String,
    /// Message discriminator and metadata without top-level text. Legacy rows may
    /// still contain complete messages; projections preserve them unchanged.
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
            created_at_ms: record.created_at_ms,
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
    #[serde(default)]
    pub catalog: Vec<WorkspaceCatalogEntryDto>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfoDto>,
    #[serde(default)]
    pub agents: Vec<AgentStateDto>,
    #[serde(default)]
    pub histories: Vec<AgentHistoryDto>,
    /// Canonical, non-sensitive driver addresses available for member creation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<ResourceIdDto>,
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
            catalog: catalog_entries(world)?,
            workspaces,
            agents: entities
                .iter()
                .map(|entity| (*entity).into_dto(world))
                .collect::<Result<_, _>>()?,
            histories: Vec::new(),
            drivers: driver_ids(world)?,
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
    requested: &ResourceIdDto,
) -> Result<(Entity, ResourceID), ProtocolError> {
    let workspace = resolve_workspace(world, reference)?;
    let component = world.get_component::<Workspace>(workspace).unwrap();
    let id = requested.clone().into_domain(())?;
    let unavailable = || {
        let detail = component
            .member_failures()
            .get(&id)
            .map(|failure| format!("; driver {}: {}", failure.driver, failure.error))
            .unwrap_or_default();
        ProtocolError::new(
            ProtocolErrorKind::AgentNotFound,
            format!(
                "agent unavailable: {id} in workspace {}{detail}",
                component.definition().id
            ),
        )
    };
    let entity = world.agent_by_id(&id).map_err(|_| unavailable())?;
    let agent = world.get_component::<Agent>(entity).unwrap();
    if agent.workspace != component.definition().id
        || !component
            .iter()
            .any(|(member, current)| member == &id && current == entity)
    {
        return Err(unavailable());
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCatalogEntryDto {
    pub id: ResourceIdDto,
    pub name: String,
    pub path: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkspaceDefinitionDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub lifecycle: String,
}
impl FromDomain<&WorkspaceCatalogEntry, &World> for WorkspaceCatalogEntryDto {
    fn from_domain(entry: &WorkspaceCatalogEntry, world: &World) -> Result<Self, ProtocolError> {
        let lifecycle = world
            .workspace_by_id(&entry.id)
            .ok()
            .and_then(|entity| world.get_component::<Workspace>(entity))
            .map(|workspace| {
                use margatroid_types::workspace_components::WorkspaceState;
                match workspace.state() {
                    WorkspaceState::Starting(_) => "starting",
                    WorkspaceState::Running => "running",
                    WorkspaceState::Stopping(_) => "stopping",
                    WorkspaceState::Failed { .. } => "failed",
                }
            })
            .unwrap_or("stopped");
        Ok(Self {
            id: (&entry.id).into_dto(())?,
            name: entry.name.clone(),
            path: entry.path.to_string_lossy().into(),
            revision: entry.revision.clone(),
            definition: entry
                .definition
                .as_ref()
                .map(|definition| definition.into_dto(()))
                .transpose()?,
            error: entry.error.clone(),
            lifecycle: lifecycle.into(),
        })
    }
}
pub fn catalog_entries(world: &World) -> Result<Vec<WorkspaceCatalogEntryDto>, ProtocolError> {
    let mut entries: Vec<WorkspaceCatalogEntryDto> = world
        .get_resource::<WorkspaceCatalog>()
        .map(|catalog| {
            catalog
                .0
                .iter()
                .map(|entry| entry.into_dto(world))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    entries.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    Ok(entries)
}
/// Canonical driver addresses published by the driver loader through [`DriverCatalog`].
///
/// Only addresses are exposed; driver sources, origins, dependencies and model settings stay
/// private. A missing resource yields an empty list so installations without a driver scan
/// keep working.
pub fn driver_ids(world: &World) -> Result<Vec<ResourceIdDto>, ProtocolError> {
    let mut drivers: Vec<ResourceIdDto> = world
        .get_resource::<DriverCatalog>()
        .map(|catalog| {
            catalog
                .0
                .iter()
                .map(|id| id.into_dto(()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    drivers.sort_by(|a, b| a.0.cmp(&b.0));
    drivers.dedup();
    Ok(drivers)
}
/// Logical workspace or member name accepted by catalog operations.
///
/// The same shape the lifecycle derives addresses from: non-empty, at most 128 bytes, not a
/// path segment (`/`, `\`, `.`, `..`) and free of control characters.
pub fn valid_logical_name(name: &str) -> Result<(), ProtocolError> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(ProtocolError::new(
            ProtocolErrorKind::InvalidRequest,
            "name must be non-empty, at most 128 bytes, and contain no path separators, control characters, '.' or '..'",
        ));
    }
    Ok(())
}
