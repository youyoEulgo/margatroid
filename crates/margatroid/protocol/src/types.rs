use crate::ProtocolErrorDto;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type AddressDto = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ClientMessage {
    #[serde(rename = "connection.register")]
    ConnectionRegister {
        id: String,
        message: RegisterConnectionDto,
    },
    #[serde(rename = "workspace.start")]
    WorkspaceStart {
        id: String,
        message: WorkspacePathDto,
    },
    #[serde(rename = "workspace.restart")]
    WorkspaceRestart {
        id: String,
        message: WorkspacePathDto,
    },
    #[serde(rename = "workspace.stop")]
    WorkspaceStop {
        id: String,
        message: WorkspaceStopDto,
    },
    #[serde(rename = "state.query")]
    StateQuery {
        id: String,
        #[serde(default)]
        message: EmptyDto,
    },
    #[serde(rename = "workspace.definitions.query")]
    WorkspaceDefinitionsQuery {
        id: String,
        #[serde(default)]
        message: EmptyDto,
    },
    #[serde(rename = "history.query")]
    HistoryQuery { id: String, message: TargetDto },
    #[serde(rename = "mcl.command")]
    MclCommand { id: String, message: MclCommandDto },
    #[serde(rename = "agent.turn.abort")]
    AgentTurnAbort {
        id: String,
        message: AgentTurnAbortDto,
    },
    #[serde(rename = "agent.start")]
    AgentStart { id: String, message: AgentStartDto },
    #[serde(rename = "agent.restart")]
    AgentRestart { id: String, message: AgentStartDto },
}

impl ClientMessage {
    pub fn id(&self) -> &str {
        match self {
            Self::ConnectionRegister { id, .. }
            | Self::WorkspaceStart { id, .. }
            | Self::WorkspaceRestart { id, .. }
            | Self::WorkspaceStop { id, .. }
            | Self::StateQuery { id, .. }
            | Self::WorkspaceDefinitionsQuery { id, .. }
            | Self::HistoryQuery { id, .. }
            | Self::MclCommand { id, .. }
            | Self::AgentTurnAbort { id, .. }
            | Self::AgentStart { id, .. }
            | Self::AgentRestart { id, .. } => id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EmptyDto {}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterConnectionDto {
    pub client_type: String,
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePathDto {
    pub path: String,
}
pub type WorkspaceStartDto = WorkspacePathDto;
pub type WorkspaceRestartDto = WorkspacePathDto;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStopDto {
    pub workspace: WorkspaceReferenceDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum WorkspaceReferenceDto {
    Address(AddressDto),
    Reference {
        id: AddressDto,
        name: String,
        project_root: String,
    },
}
impl WorkspaceReferenceDto {
    pub fn address(&self) -> &str {
        match self {
            Self::Address(value) => value,
            Self::Reference { id, .. } => id,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: AddressDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MclCommandDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: AddressDto,
    pub command: String,
    #[serde(default)]
    pub arguments: Vec<Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurnAbortDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: AddressDto,
    pub agent_instance: String,
    pub turn_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStartDto {
    pub workspace: AddressDto,
    pub name: String,
    pub parameters: AgentStartParametersDto,
}
pub type AgentRestartDto = AgentStartDto;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStartParametersDto {
    pub driver: String,
    pub home_root: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "connection.registered")]
    ConnectionRegistered { id: String, client: ClientInfoDto },
    #[serde(rename = "connection.register_failed")]
    ConnectionRegisterFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "request.failed")]
    RequestFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "state.sync")]
    StateSync { state: BackendStateDto },
    #[serde(rename = "workspace.definitions")]
    WorkspaceDefinitions {
        id: String,
        definitions: Vec<WorkspaceConfigDto>,
    },
    #[serde(rename = "workspace.started")]
    WorkspaceStarted {
        id: String,
        workspace: WorkspaceInfoDto,
    },
    #[serde(rename = "workspace.restarted")]
    WorkspaceRestarted {
        id: String,
        workspace: WorkspaceInfoDto,
    },
    #[serde(rename = "workspace.stopped")]
    WorkspaceStopped {
        id: String,
        workspace: WorkspaceReferenceDto,
    },
    #[serde(rename = "workspace.start_failed")]
    WorkspaceStartFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "workspace.restart_failed")]
    WorkspaceRestartFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "workspace.stop_failed")]
    WorkspaceStopFailed { id: String, error: ProtocolErrorDto },
    #[serde(rename = "agent.started")]
    AgentStarted { id: String, agent: AddressDto },
    #[serde(rename = "agent.restarted")]
    AgentRestarted { id: String, agent: AddressDto },
    #[serde(rename = "history.result")]
    HistoryResult {
        id: String,
        history: AgentHistoryDto,
    },
    #[serde(rename = "mcl.command_result")]
    MclCommandResult {
        id: String,
        source: AddressDto,
        target: AddressDto,
        result: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        history_error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.turn.abort_result")]
    AgentTurnAbortResult {
        id: String,
        workspace: WorkspaceReferenceDto,
        agent: AddressDto,
        turn_id: String,
        result: Value,
    },
    #[serde(rename = "agent.message.delta")]
    AgentMessageDelta {
        id: String,
        agent: AddressDto,
        agent_instance: String,
        sequence: u64,
        content: String,
    },
    #[serde(rename = "agent.message.reasoning_delta")]
    AgentMessageReasoningDelta {
        id: String,
        agent: AddressDto,
        agent_instance: String,
        sequence: u64,
        content: String,
    },
    #[serde(rename = "agent.message.finished")]
    AgentMessageFinished {
        id: String,
        agent: AddressDto,
        agent_instance: String,
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "log")]
    Log { record: LogRecordDto },
}

impl ServerMessage {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::StateSync { .. } | Self::Log { .. } => None,
            Self::ConnectionRegistered { id, .. }
            | Self::ConnectionRegisterFailed { id, .. }
            | Self::RequestFailed { id, .. }
            | Self::WorkspaceDefinitions { id, .. }
            | Self::WorkspaceStarted { id, .. }
            | Self::WorkspaceRestarted { id, .. }
            | Self::WorkspaceStopped { id, .. }
            | Self::WorkspaceStartFailed { id, .. }
            | Self::WorkspaceRestartFailed { id, .. }
            | Self::WorkspaceStopFailed { id, .. }
            | Self::AgentStarted { id, .. }
            | Self::AgentRestarted { id, .. }
            | Self::HistoryResult { id, .. }
            | Self::MclCommandResult { id, .. }
            | Self::AgentTurnAbortResult { id, .. }
            | Self::AgentMessageDelta { id, .. }
            | Self::AgentMessageReasoningDelta { id, .. }
            | Self::AgentMessageFinished { id, .. } => Some(id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientInfoDto {
    pub resource_id: AddressDto,
    pub client_type: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct BackendStateDto {
    pub workspace_definitions: Vec<WorkspaceConfigDto>,
    pub workspaces: Vec<WorkspaceInfoDto>,
    pub agents: Vec<AgentStateDto>,
    pub histories: Vec<AgentHistoryDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceConfigDto {
    pub path: String,
    pub cache: WorkspaceDefinitionDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDefinitionDto {
    pub name: String,
    pub project_root: String,
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub driver: String,
    pub home_root: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceMemberFailureDto {
    pub agent: String,
    pub driver: String,
    pub error: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    pub id: AddressDto,
    pub name: String,
    pub project_root: String,
    pub definition_path: String,
    pub status: String,
    pub agents: Vec<AddressDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_failures: Vec<WorkspaceMemberFailureDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStateDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: AddressDto,
    pub agent_instance: String,
    pub status: String,
    pub turn: Option<String>,
    pub sandbox: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub sandboxes: Vec<String>,
    pub queued_commands: usize,
    pub pending_command: Option<String>,
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolErrorDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentHistoryDto {
    pub workspace: WorkspaceReferenceDto,
    pub agent: AddressDto,
    pub messages: Vec<HistoryMessageDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryMessageDto {
    pub sequence: u64,
    pub turn: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub message_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub kind: String,
    pub content: String,
    pub payload: Value,
    pub created_at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogFieldDto {
    pub name: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecordDto {
    pub timestamp_millis: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: Vec<LogFieldDto>,
    #[serde(default)]
    pub spans: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserImageDto {
    pub media_type: String,
    pub data: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDto {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenUsageDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReasonDto {
    Completed,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessageDto {
    System {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        content: String,
    },
    Inject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        messages: Vec<AgentMessageDto>,
    },
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        content: String,
        #[serde(default, rename = "data")]
        images: Vec<UserImageDto>,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        reasoning: Option<String>,
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCallDto>,
        usage: Option<TokenUsageDto>,
        stop_reason: Option<StopReasonDto>,
    },
    ToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        tool_call_id: String,
        tool_name: String,
        content: String,
        #[serde(default, rename = "data")]
        images: Vec<UserImageDto>,
        failed: bool,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        message: String,
    },
}
