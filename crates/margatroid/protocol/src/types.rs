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
    #[serde(rename = "workspace.req")]
    WorkspaceReq { id: String, message: EmptyDto },
    #[serde(rename = "workspace.start")]
    WorkspaceStart {
        id: String,
        message: WorkspacePathDto,
    },
    #[serde(rename = "workspace.stop")]
    WorkspaceStop {
        id: String,
        message: WorkspacePathDto,
    },
    #[serde(rename = "workspace.restart")]
    WorkspaceRestart {
        id: String,
        message: WorkspacePathDto,
    },
    #[serde(rename = "workspace.reload")]
    WorkspaceReload { id: String, message: EmptyDto },
    #[serde(rename = "agent.req")]
    AgentReq { id: String, message: EmptyDto },
    #[serde(rename = "agent.start")]
    AgentStart { id: String, message: AgentStartDto },
    #[serde(rename = "agent.stop")]
    AgentStop { id: String, message: AgentTargetDto },
    #[serde(rename = "agent.restart")]
    AgentRestart { id: String, message: AgentTargetDto },
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
            Self::WorkspaceReq { id, .. } => id,
            Self::WorkspaceStart { id, .. } => id,
            Self::WorkspaceStop { id, .. } => id,
            Self::WorkspaceRestart { id, .. } => id,
            Self::WorkspaceReload { id, .. } => id,
            Self::AgentReq { id, .. } => id,
            Self::AgentStart { id, .. } => id,
            Self::AgentStop { id, .. } => id,
            Self::AgentRestart { id, .. } => id,
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
    WorkspaceRes {
        id: String,
        success: bool,
        workspaces: Option<Vec<WorkspaceInfoDto>>,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceSync {
        workspaces: Vec<WorkspaceInfoDto>,
    },
    WorkspaceStartResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceStopResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceRestartResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
    },
    WorkspaceReloadResult {
        id: String,
        success: bool,
        error: Option<ProtocolErrorDto>,
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
    AgentRestartResult {
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

        turn: String,
        generation: String,
        sequence: u64,
        delta: MessageDeltaDto,
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
enum ServerWire {
    #[serde(rename = "connection.register.result")]
    ConnectionRegisterResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<ClientInfoDto>,
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
    #[serde(rename = "workspace.start.result")]
    WorkspaceStartResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.stop.result")]
    WorkspaceStopResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.restart.result")]
    WorkspaceRestartResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
    #[serde(rename = "workspace.reload.result")]
    WorkspaceReloadResult {
        id: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolErrorDto>,
    },
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
    #[serde(rename = "agent.restart.result")]
    AgentRestartResult {
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

        turn: String,
        generation: String,
        sequence: u64,
        delta: MessageDeltaDto,
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
            ServerMessage::WorkspaceStartResult { id, success, error } => {
                Self::WorkspaceStartResult { id, success, error }
            }
            ServerMessage::WorkspaceStopResult { id, success, error } => {
                Self::WorkspaceStopResult { id, success, error }
            }
            ServerMessage::WorkspaceRestartResult { id, success, error } => {
                Self::WorkspaceRestartResult { id, success, error }
            }
            ServerMessage::WorkspaceReloadResult { id, success, error } => {
                Self::WorkspaceReloadResult { id, success, error }
            }
            ServerMessage::AgentStartResult { id, success, error } => {
                Self::AgentStartResult { id, success, error }
            }
            ServerMessage::AgentStopResult { id, success, error } => {
                Self::AgentStopResult { id, success, error }
            }
            ServerMessage::AgentRestartResult { id, success, error } => {
                Self::AgentRestartResult { id, success, error }
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
                delta,
            } => Self::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                delta,
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
            ServerWire::WorkspaceStartResult { id, success, error } => {
                Self::WorkspaceStartResult { id, success, error }
            }
            ServerWire::WorkspaceStopResult { id, success, error } => {
                Self::WorkspaceStopResult { id, success, error }
            }
            ServerWire::WorkspaceRestartResult { id, success, error } => {
                Self::WorkspaceRestartResult { id, success, error }
            }
            ServerWire::WorkspaceReloadResult { id, success, error } => {
                Self::WorkspaceReloadResult { id, success, error }
            }
            ServerWire::AgentStartResult { id, success, error } => {
                Self::AgentStartResult { id, success, error }
            }
            ServerWire::AgentStopResult { id, success, error } => {
                Self::AgentStopResult { id, success, error }
            }
            ServerWire::AgentRestartResult { id, success, error } => {
                Self::AgentRestartResult { id, success, error }
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
                delta,
            } => Self::AgentMessageChunkSync {
                address,

                turn,
                generation,
                sequence,
                delta,
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
            Self::ConnectionRegisterResult { id, .. } => Some(id),
            Self::WorkspaceRes { id, .. } => Some(id),
            Self::WorkspaceSync { .. } => None,
            Self::WorkspaceStartResult { id, .. } => Some(id),
            Self::WorkspaceStopResult { id, .. } => Some(id),
            Self::WorkspaceRestartResult { id, .. } => Some(id),
            Self::WorkspaceReloadResult { id, .. } => Some(id),
            Self::AgentStartResult { id, .. } => Some(id),
            Self::AgentStopResult { id, .. } => Some(id),
            Self::AgentRestartResult { id, .. } => Some(id),
            Self::AgentRes { id, .. } => Some(id),
            Self::AgentSync { .. } => None,
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
pub struct WorkspacePathDto {
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStartDto {
    pub workspace: AddressDto,
    pub name: String,
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
    pub path: String,
    pub definition: WorkspaceDefinitionDto,
    pub status: String,
    pub address: Option<AddressDto>,
    pub error: Option<ProtocolErrorDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDefinitionDto {
    pub name: String,
    pub project_root: String,
    pub agents: Vec<WorkspaceAgentDefinitionDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAgentDefinitionDto {
    pub name: String,
    pub driver: String,
    pub home_root: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStateDto {
    pub address: AddressDto,
    pub workspace: AddressDto,

    pub status: String,
    pub tools: Vec<AgentToolDto>,
    pub error: Option<ProtocolErrorDto>,
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
pub struct MessageDeltaDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
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
            Self::Assistant { sequence, .. } => *sequence,
            Self::ToolCall { sequence, .. } => *sequence,
            Self::Inject { sequence, .. } => *sequence,
            Self::Error { sequence, .. } => *sequence,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MclExecuteDataDto {
    Messages { value: Vec<AgentMessageDto> },
    Config { value: Value },
    Import { value: MclImportResultDto },
    Inference { value: MclInferenceResultDto },
    BlockSnapshots { value: Value },
}
