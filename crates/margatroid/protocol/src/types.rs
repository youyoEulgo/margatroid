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
    #[serde(rename = "model.config.req")]
    ModelConfigReq { id: String, message: EmptyDto },
    #[serde(rename = "model.config.update")]
    ModelConfigUpdate {
        id: String,
        message: ModelConfigUpdateDto,
    },
    #[serde(rename = "resource.req")]
    ResourceReq {
        id: String,
        message: ResourceQueryDto,
    },
    #[serde(rename = "workspace.req")]
    WorkspaceReq { id: String, message: EmptyDto },
    #[serde(rename = "agent.start")]
    AgentStart {
        id: String,
        message: AgentStartInputDto,
    },
    #[serde(rename = "agent.stop")]
    AgentStop { id: String, message: AgentTargetDto },
    #[serde(rename = "agent.archive")]
    AgentArchive {
        id: String,
        message: AgentArchiveInputDto,
    },
    #[serde(rename = "workspace.create")]
    WorkspaceCreate {
        id: String,
        message: WorkspaceCreateDto,
    },
    #[serde(rename = "workspace.delete")]
    WorkspaceDelete {
        id: String,
        message: WorkspaceTargetDto,
    },
    #[serde(rename = "agent.req")]
    AgentReq { id: String, message: EmptyDto },
    #[serde(rename = "agent.ui.req")]
    AgentUiReq { id: String, message: AgentTargetDto },
    #[serde(rename = "agent.turn.req")]
    AgentTurnReq { id: String, message: AgentTargetDto },
    #[serde(rename = "agent.turn.abort")]
    AgentTurnAbort {
        id: String,
        message: AgentTurnAbortDto,
    },
    #[serde(rename = "agent.task.req")]
    AgentTaskReq { id: String, message: AgentTargetDto },
    #[serde(rename = "agent.history.req")]
    AgentHistoryReq {
        id: String,
        message: AgentRecordsQueryDto,
    },
    #[serde(rename = "agent.log.req")]
    AgentLogReq {
        id: String,
        message: AgentRecordsQueryDto,
    },
    #[serde(rename = "agent.mcl.req")]
    AgentMclReq {
        id: String,
        message: AgentRecordsQueryDto,
    },
    #[serde(rename = "mcl.execute")]
    MclExecute { id: String, message: MclExecuteDto },
}
impl ClientMessage {
    pub fn id(&self) -> &str {
        match self {
            Self::ConnectionRegister { id, .. } => id,
            Self::ModelConfigReq { id, .. } | Self::ModelConfigUpdate { id, .. } => id,
            Self::ResourceReq { id, .. } => id,
            Self::WorkspaceReq { id, .. } => id,
            Self::AgentStart { id, .. } => id,
            Self::AgentStop { id, .. } => id,
            Self::AgentArchive { id, .. } => id,
            Self::WorkspaceCreate { id, .. } | Self::WorkspaceDelete { id, .. } => id,
            Self::AgentReq { id, .. } => id,
            Self::AgentUiReq { id, .. } => id,
            Self::AgentTurnReq { id, .. } => id,
            Self::AgentTurnAbort { id, .. } => id,
            Self::AgentTaskReq { id, .. } => id,
            Self::AgentHistoryReq { id, .. } => id,
            Self::AgentLogReq { id, .. } => id,
            Self::AgentMclReq { id, .. } => id,
            Self::MclExecute { id, .. } => id,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum ServerMessage {
    ConnectionRegisterResult {
        id: String,
        success: bool,
        client: Option<ClientInfoDto>,
        error: Option<ProtocolErrorDto>,
    },
    ModelConfigRes {
        id: String,
        success: bool,
        content: Option<String>,
        error: Option<ProtocolErrorDto>,
    },
    ModelConfigSync {
        content: Option<String>,
        error: Option<ProtocolErrorDto>,
    },
    ModelConfigUpdateResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    ResourceRes {
        id: String,
        success: bool,
        resources: Option<Vec<ResourceInfoDto>>,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceRes {
        id: String,
        success: bool,
        workspaces: Option<Vec<WorkspaceInfoDto>>,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceSync {
        workspaces: Vec<WorkspaceInfoDto>,
    },
    AgentStartResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    AgentStopResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    AgentArchiveResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceCreateResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceDeleteResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    AgentRes {
        id: String,
        success: bool,
        agents: Option<Vec<AgentStateDto>>,
        error: Option<ProtocolErrorDto>,
    },
    AgentSync {
        agents: Vec<AgentStateDto>,
    },
    AgentUiRes {
        id: String,
        success: bool,
        address: AddressDto,
        ui: Option<AgentUiBundleDto>,
        error: Option<ProtocolErrorDto>,
    },
    AgentUiSync {
        address: AddressDto,
        ui: Option<AgentUiBundleDto>,
        error: Option<ProtocolErrorDto>,
    },
    AgentTurnRes {
        id: String,
        success: bool,
        state: Option<AgentTurnStateDto>,
        error: Option<ProtocolErrorDto>,
    },
    AgentTurnSync {
        address: AddressDto,

        turn: Option<String>,
    },
    AgentTurnAbortResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    AgentTaskRes {
        id: String,
        success: bool,
        address: AddressDto,

        tasks: Option<Vec<AgentTaskDto>>,
        error: Option<ProtocolErrorDto>,
    },
    AgentTaskSync {
        address: AddressDto,

        tasks: Vec<AgentTaskDto>,
    },
    AgentHistoryRes {
        id: String,
        success: bool,
        address: AddressDto,

        records: Option<Vec<HistoryMessageDto>>,
        next_before_sequence: Option<u64>,
        next_after_sequence: Option<u64>,
        error: Option<ProtocolErrorDto>,
    },
    AgentLogRes {
        id: String,
        success: bool,
        address: AddressDto,

        records: Option<Vec<AgentLogRecordDto>>,
        next_before_sequence: Option<u64>,
        next_after_sequence: Option<u64>,
        error: Option<ProtocolErrorDto>,
    },
    AgentLogSync {
        address: AddressDto,

        records: Vec<AgentLogRecordDto>,
    },
    AgentMclRes {
        id: String,
        success: bool,
        address: AddressDto,

        records: Option<Vec<AgentMclRecordDto>>,
        next_before_sequence: Option<u64>,
        next_after_sequence: Option<u64>,
        error: Option<ProtocolErrorDto>,
    },
    AgentMclSync {
        address: AddressDto,

        records: Vec<AgentMclRecordDto>,
    },
    AgentMessageChunkSync {
        address: AddressDto,

        turn: Option<String>,
        generation: String,
        sequence: u64,
        content: String,
        reasoning: String,
    },
    AgentMessageFullSync {
        address: AddressDto,

        turn: Option<String>,
        generation: Option<String>,
        history_sequence: Option<u64>,
        message: AgentMessageDto,
        error: Option<ProtocolErrorDto>,
    },
    MclExecuteResult {
        id: String,
        success: bool,
        data: Option<MclExecuteDataDto>,
        error: Option<ProtocolErrorDto>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(crate) enum ServerWire {
    #[serde(rename = "connection.register.result")]
    ConnectionRegisterResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<ClientInfoDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "model.config.res")]
    ModelConfigRes {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "model.config.sync")]
    ModelConfigSync {
        content: Option<String>,
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "model.config.update.result")]
    ModelConfigUpdateResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "resource.res")]
    ResourceRes {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resources: Option<Vec<ResourceInfoDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.res")]
    WorkspaceRes {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspaces: Option<Vec<WorkspaceInfoDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.sync")]
    WorkspaceSync { workspaces: Vec<WorkspaceInfoDto> },
    #[serde(rename = "agent.start.result")]
    AgentStartResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.stop.result")]
    AgentStopResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.archive.result")]
    AgentArchiveResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.create.result")]
    WorkspaceCreateResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.delete.result")]
    WorkspaceDeleteResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.res")]
    AgentRes {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<AgentStateDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.sync")]
    AgentSync { agents: Vec<AgentStateDto> },
    #[serde(rename = "agent.ui.res")]
    AgentUiRes {
        id: String,
        success: bool,
        address: AddressDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui: Option<AgentUiBundleDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.ui.sync")]
    AgentUiSync {
        address: AddressDto,
        ui: Option<AgentUiBundleDto>,
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.turn.res")]
    AgentTurnRes {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<AgentTurnStateDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.turn.sync")]
    AgentTurnSync {
        address: AddressDto,

        turn: Option<String>,
    },
    #[serde(rename = "agent.turn.abort.result")]
    AgentTurnAbortResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.task.res")]
    AgentTaskRes {
        id: String,
        success: bool,
        address: AddressDto,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        tasks: Option<Vec<AgentTaskDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.task.sync")]
    AgentTaskSync {
        address: AddressDto,

        tasks: Vec<AgentTaskDto>,
    },
    #[serde(rename = "agent.history.res")]
    AgentHistoryRes {
        id: String,
        success: bool,
        address: AddressDto,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Vec<HistoryMessageDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_before_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_after_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.log.res")]
    AgentLogRes {
        id: String,
        success: bool,
        address: AddressDto,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Vec<AgentLogRecordDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_before_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_after_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.log.sync")]
    AgentLogSync {
        address: AddressDto,

        records: Vec<AgentLogRecordDto>,
    },
    #[serde(rename = "agent.mcl.res")]
    AgentMclRes {
        id: String,
        success: bool,
        address: AddressDto,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Vec<AgentMclRecordDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_before_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_after_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "agent.mcl.sync")]
    AgentMclSync {
        address: AddressDto,

        records: Vec<AgentMclRecordDto>,
    },
    #[serde(rename = "agent.message.chunk.sync")]
    AgentMessageChunkSync {
        address: AddressDto,

        turn: Option<String>,
        generation: String,
        sequence: u64,
        content: String,
        reasoning: String,
    },
    #[serde(rename = "agent.message.full.sync")]
    AgentMessageFullSync {
        address: AddressDto,

        turn: Option<String>,
        generation: Option<String>,
        history_sequence: Option<u64>,
        message: AgentMessageDto,
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "mcl.execute.result")]
    MclExecuteResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<MclExecuteDataDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
}
impl From<ServerMessage> for ServerWire {
    fn from(value: ServerMessage) -> Self {
        match value {
            ServerMessage::ConnectionRegisterResult {
                id,
                success,
                client,
                error,
            } => Self::ConnectionRegisterResult {
                id,
                success,
                client,
                error,
            },
            ServerMessage::ModelConfigRes {
                id,
                success,
                content,
                error,
            } => Self::ModelConfigRes {
                id,
                success,
                content,
                error,
            },
            ServerMessage::ModelConfigSync { content, error } => {
                Self::ModelConfigSync { content, error }
            }
            ServerMessage::ModelConfigUpdateResult { id, success, error } => {
                Self::ModelConfigUpdateResult { id, success, error }
            }
            ServerMessage::ResourceRes {
                id,
                success,
                resources,
                error,
            } => Self::ResourceRes {
                id,
                success,
                resources,
                error,
            },
            ServerMessage::WorkspaceRes {
                id,
                success,
                workspaces,
                error,
            } => Self::WorkspaceRes {
                id,
                success,
                workspaces,
                error,
            },
            ServerMessage::WorkspaceSync { workspaces } => Self::WorkspaceSync { workspaces },
            ServerMessage::AgentStartResult { id, success, error } => {
                Self::AgentStartResult { id, success, error }
            }
            ServerMessage::AgentStopResult { id, success, error } => {
                Self::AgentStopResult { id, success, error }
            }
            ServerMessage::AgentArchiveResult { id, success, error } => {
                Self::AgentArchiveResult { id, success, error }
            }
            ServerMessage::WorkspaceCreateResult { id, success, error } => {
                Self::WorkspaceCreateResult { id, success, error }
            }
            ServerMessage::WorkspaceDeleteResult { id, success, error } => {
                Self::WorkspaceDeleteResult { id, success, error }
            }
            ServerMessage::AgentRes {
                id,
                success,
                agents,
                error,
            } => Self::AgentRes {
                id,
                success,
                agents,
                error,
            },
            ServerMessage::AgentSync { agents } => Self::AgentSync { agents },
            ServerMessage::AgentUiRes {
                id,
                success,
                address,
                ui,
                error,
            } => Self::AgentUiRes {
                id,
                success,
                address,
                ui,
                error,
            },
            ServerMessage::AgentUiSync { address, ui, error } => {
                Self::AgentUiSync { address, ui, error }
            }
            ServerMessage::AgentTurnRes {
                id,
                success,
                state,
                error,
            } => Self::AgentTurnRes {
                id,
                success,
                state,
                error,
            },
            ServerMessage::AgentTurnSync { address, turn } => Self::AgentTurnSync { address, turn },
            ServerMessage::AgentTurnAbortResult { id, success, error } => {
                Self::AgentTurnAbortResult { id, success, error }
            }
            ServerMessage::AgentTaskRes {
                id,
                success,
                address,

                tasks,
                error,
            } => Self::AgentTaskRes {
                id,
                success,
                address,

                tasks,
                error,
            },
            ServerMessage::AgentTaskSync { address, tasks } => {
                Self::AgentTaskSync { address, tasks }
            }
            ServerMessage::AgentHistoryRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentHistoryRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerMessage::AgentLogRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentLogRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerMessage::AgentLogSync { address, records } => {
                Self::AgentLogSync { address, records }
            }
            ServerMessage::AgentMclRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentMclRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerMessage::AgentMclSync { address, records } => {
                Self::AgentMclSync { address, records }
            }
            ServerMessage::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                content,
                reasoning,
            } => Self::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                content,
                reasoning,
            },
            ServerMessage::AgentMessageFullSync {
                address,

                turn,
                generation,
                history_sequence,
                message,
                error,
            } => Self::AgentMessageFullSync {
                address,

                turn,
                generation,
                history_sequence,
                message,
                error,
            },
            ServerMessage::MclExecuteResult {
                id,
                success,
                data,
                error,
            } => Self::MclExecuteResult {
                id,
                success,
                data,
                error,
            },
        }
    }
}
impl From<ServerWire> for ServerMessage {
    fn from(value: ServerWire) -> Self {
        match value {
            ServerWire::ConnectionRegisterResult {
                id,
                success,
                client,
                error,
            } => Self::ConnectionRegisterResult {
                id,
                success,
                client,
                error,
            },
            ServerWire::ModelConfigRes {
                id,
                success,
                content,
                error,
            } => Self::ModelConfigRes {
                id,
                success,
                content,
                error,
            },
            ServerWire::ModelConfigSync { content, error } => {
                Self::ModelConfigSync { content, error }
            }
            ServerWire::ModelConfigUpdateResult { id, success, error } => {
                Self::ModelConfigUpdateResult { id, success, error }
            }
            ServerWire::ResourceRes {
                id,
                success,
                resources,
                error,
            } => Self::ResourceRes {
                id,
                success,
                resources,
                error,
            },
            ServerWire::WorkspaceRes {
                id,
                success,
                workspaces,
                error,
            } => Self::WorkspaceRes {
                id,
                success,
                workspaces,
                error,
            },
            ServerWire::WorkspaceSync { workspaces } => Self::WorkspaceSync { workspaces },
            ServerWire::AgentStartResult { id, success, error } => {
                Self::AgentStartResult { id, success, error }
            }
            ServerWire::AgentStopResult { id, success, error } => {
                Self::AgentStopResult { id, success, error }
            }
            ServerWire::AgentArchiveResult { id, success, error } => {
                Self::AgentArchiveResult { id, success, error }
            }
            ServerWire::WorkspaceCreateResult { id, success, error } => {
                Self::WorkspaceCreateResult { id, success, error }
            }
            ServerWire::WorkspaceDeleteResult { id, success, error } => {
                Self::WorkspaceDeleteResult { id, success, error }
            }
            ServerWire::AgentRes {
                id,
                success,
                agents,
                error,
            } => Self::AgentRes {
                id,
                success,
                agents,
                error,
            },
            ServerWire::AgentSync { agents } => Self::AgentSync { agents },
            ServerWire::AgentUiRes {
                id,
                success,
                address,
                ui,
                error,
            } => Self::AgentUiRes {
                id,
                success,
                address,
                ui,
                error,
            },
            ServerWire::AgentUiSync { address, ui, error } => {
                Self::AgentUiSync { address, ui, error }
            }
            ServerWire::AgentTurnRes {
                id,
                success,
                state,
                error,
            } => Self::AgentTurnRes {
                id,
                success,
                state,
                error,
            },
            ServerWire::AgentTurnSync { address, turn } => Self::AgentTurnSync { address, turn },
            ServerWire::AgentTurnAbortResult { id, success, error } => {
                Self::AgentTurnAbortResult { id, success, error }
            }
            ServerWire::AgentTaskRes {
                id,
                success,
                address,

                tasks,
                error,
            } => Self::AgentTaskRes {
                id,
                success,
                address,

                tasks,
                error,
            },
            ServerWire::AgentTaskSync { address, tasks } => Self::AgentTaskSync { address, tasks },
            ServerWire::AgentHistoryRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentHistoryRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerWire::AgentLogRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentLogRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerWire::AgentLogSync { address, records } => {
                Self::AgentLogSync { address, records }
            }
            ServerWire::AgentMclRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            } => Self::AgentMclRes {
                id,
                success,
                address,

                records,
                next_before_sequence,
                next_after_sequence,
                error,
            },
            ServerWire::AgentMclSync { address, records } => {
                Self::AgentMclSync { address, records }
            }
            ServerWire::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                content,
                reasoning,
            } => Self::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                content,
                reasoning,
            },
            ServerWire::AgentMessageFullSync {
                address,

                turn,
                generation,
                history_sequence,
                message,
                error,
            } => Self::AgentMessageFullSync {
                address,

                turn,
                generation,
                history_sequence,
                message,
                error,
            },
            ServerWire::MclExecuteResult {
                id,
                success,
                data,
                error,
            } => Self::MclExecuteResult {
                id,
                success,
                data,
                error,
            },
        }
    }
}
impl Serialize for ServerMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = serde_json::to_value(ServerWire::from(self.clone()))
            .map_err(serde::ser::Error::custom)?;
        if value.get("success") == Some(&Value::Bool(true))
            && matches!(
                value["type"].as_str(),
                Some("agent.history.res" | "agent.log.res" | "agent.mcl.res")
            )
        {
            for key in ["next_before_sequence", "next_after_sequence"] {
                value
                    .as_object_mut()
                    .unwrap()
                    .entry(key)
                    .or_insert(Value::Null);
            }
        }
        if value["type"] == "agent.ui.res" && value["success"] == true {
            value
                .as_object_mut()
                .unwrap()
                .entry("ui")
                .or_insert(Value::Null);
        }
        crate::validate_server_value(&value).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ServerMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        crate::validate_server_value(&value).map_err(serde::de::Error::custom)?;
        serde_json::from_value::<ServerWire>(value)
            .map(Into::into)
            .map_err(serde::de::Error::custom)
    }
}
impl ServerMessage {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::ModelConfigRes { id, .. } | Self::ModelConfigUpdateResult { id, .. } => Some(id),
            Self::ModelConfigSync { .. } => None,
            Self::ConnectionRegisterResult { id, .. } => Some(id),
            Self::ResourceRes { id, .. } => Some(id),
            Self::WorkspaceRes { id, .. } => Some(id),
            Self::WorkspaceSync { .. } => None,
            Self::AgentStartResult { id, .. } => Some(id),
            Self::AgentStopResult { id, .. } => Some(id),
            Self::AgentArchiveResult { id, .. } => Some(id),
            Self::WorkspaceCreateResult { id, .. } | Self::WorkspaceDeleteResult { id, .. } => {
                Some(id)
            }
            Self::AgentRes { id, .. } => Some(id),
            Self::AgentSync { .. } => None,
            Self::AgentUiRes { id, .. } => Some(id),
            Self::AgentUiSync { .. } => None,
            Self::AgentTurnRes { id, .. } => Some(id),
            Self::AgentTurnSync { .. } => None,
            Self::AgentTurnAbortResult { id, .. } => Some(id),
            Self::AgentTaskRes { id, .. } => Some(id),
            Self::AgentTaskSync { .. } => None,
            Self::AgentHistoryRes { id, .. } => Some(id),
            Self::AgentLogRes { id, .. } => Some(id),
            Self::AgentLogSync { .. } => None,
            Self::AgentMclRes { id, .. } => Some(id),
            Self::AgentMclSync { .. } => None,
            Self::AgentMessageChunkSync { .. } => None,
            Self::AgentMessageFullSync { .. } => None,
            Self::MclExecuteResult { id, .. } => Some(id),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyDto {}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterConnectionDto {
    pub client_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientInfoDto {
    pub address: AddressDto,
    pub client_type: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTargetDto {
    pub address: AddressDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTargetDto {
    pub address: AddressDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurnAbortDto {
    pub address: AddressDto,

    pub turn: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRecordsQueryDto {
    pub address: AddressDto,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MclExecuteDto {
    pub address: AddressDto,

    pub command: String,
    #[serde(default)]
    pub arguments: Vec<Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInfoDto {
    pub address: AddressDto,
    pub alias: String,
    pub project_root: String,
    pub status: String,
    pub agents: Vec<WorkspaceAgentInfoDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAgentInfoDto {
    pub id: String,
    pub parent_node: AddressDto,
    pub node_name: Option<String>,
    pub alias: Option<String>,
    pub category: String,
    pub tool_path: String,
    pub tool_arguments: Value,
    pub previous_node_name: Option<String>,
    pub previous_alias: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStateDto {
    pub address: AddressDto,
    pub workspace: AddressDto,

    pub creator: AgentCreatorDto,
    pub status: String,
    pub tools: Vec<AgentToolDto>,
    pub sandboxes: Vec<AgentSandboxDto>,
    pub sandbox: Option<String>,
    pub model: Option<AgentModelDto>,
    pub error: Option<ProtocolErrorDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSandboxDto {
    pub alias: String,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentModelDto {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub api_type: String,
    pub context_window_tokens: u64,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentToolDto {
    pub alias: String,
    pub path: String,
    pub description: String,
    pub input_schema: Value,
    pub owner_task_id: Option<String>,
    pub binding: Option<Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurnStateDto {
    pub address: AddressDto,

    pub turn: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTaskStateDto {
    pub address: AddressDto,

    pub tasks: Vec<AgentTaskDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTaskDto {
    pub task_id: String,
    pub call_id: String,
    pub alias: String,
    pub returned: bool,
    pub output: Vec<Value>,
    pub error: Option<ProtocolErrorDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserImageDto {
    pub media_type: String,
    pub data: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallDto {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenUsageDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryMessageDto {
    pub sequence: u64,
    pub turn: Option<String>,
    pub kind: String,
    pub content: String,
    pub payload: AgentMessageDto,
    pub created_at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLogRecordDto {
    pub sequence: u64,
    pub created_at_ms: u64,
    pub level: String,
    pub source: String,
    pub stage: String,
    pub message: String,
    pub details_json: Option<Value>,
    pub context_json: Option<Value>,
    pub origin: Option<String>,
    pub raw_error: Option<String>,
    pub traceback: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMclRecordDto {
    pub sequence: u64,
    pub created_at_ms: u64,
    pub source: AddressDto,
    pub turn: Option<String>,
    pub content: String,
    pub payload: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MclImportResultDto {
    pub kind: String,
    pub alias: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MclInferenceResultDto {
    pub message: AgentMessageDto,
    pub stop_reason: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMessageDto {
    System {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        content: String,
    },
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        content: String,
        #[serde(default)]
        data: Vec<UserImageDto>,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCallDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsageDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    Compact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCallDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsageDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    ToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        tool_call_id: String,
        tool_name: String,
        content: String,
        #[serde(default)]
        data: Vec<UserImageDto>,
        failed: bool,
    },
    Inject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        #[serde(default)]
        messages: Vec<AgentMessageDto>,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
}
impl AgentMessageDto {
    pub fn sequence(&self) -> Option<u64> {
        match self {
            Self::System { sequence, .. } => *sequence,
            Self::User { sequence, .. } => *sequence,
            Self::Assistant { sequence, .. } | Self::Compact { sequence, .. } => *sequence,
            Self::ToolCall { sequence, .. } => *sequence,
            Self::Inject { sequence, .. } => *sequence,
            Self::Error { sequence, .. } => *sequence,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MclExecuteDataDto {
    Turn { value: String },
    Messages { value: Vec<AgentMessageDto> },
    Config { value: Value },
    Import { value: MclImportResultDto },
    Inference { value: MclInferenceResultDto },
    BlockSnapshots { value: Value },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUiBundleDto {
    pub revision: String,
    pub entry: String,
    pub files: Vec<AgentUiFileDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUiFileDto {
    pub path: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfigUpdateDto {
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCreatorDto {
    pub address: AddressDto,
    pub task_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateDto {
    pub address: AddressDto,
    pub alias: String,
    pub project_root: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentStartInputDto {
    Existing {
        workspace: AddressDto,
        node_name: String,
    },
    New {
        workspace: AddressDto,
        tool_path: String,
        arguments: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentArchiveInputDto {
    pub workspace: AddressDto,
    pub node_name: String,
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceQueryDto {
    #[serde(
        rename = "type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    pub resource_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    pub scope: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceInfoDto {
    pub address: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub scope: String,
    pub name: String,
    pub tag: String,
    pub path: String,
    pub tool_path: Option<String>,
}
